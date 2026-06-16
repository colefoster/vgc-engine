//! OracleRng event extraction from PS replays.
//!
//! Walks a replay's event stream and produces a `Vec<RngEvent>` that
//! the engine can replay via `Rng::oracle_partial(...)`. Phase-2
//! scope is the highest-impact channel only: **crit hit/miss**, one
//! per damaging move. Damage-roll back-solving and percent-roll
//! extraction land in their own PRs once this proves out the wiring.
//!
//! ## Why crit alone is worth a PR
//!
//! Crit doubles damage and ignores positive defensive boosts. A single
//! mis-classified crit on a key hit can swing a turn's HP fraction by
//! 30-50%, which is well past the corpus harness's ±5% agreement
//! tolerance. The engine's splitmix64 RNG is independent of PS's
//! battle PRNG, so on any reasonably long battle the two streams agree
//! by chance — every recorded crit we feed in turns a "maybe crit"
//! into a deterministic match against PS.
//!
//! ## Protocol shape we read
//!
//! ```text
//! |move|p1a: Garchomp|Dragon Claw|p2a: Pelipper
//! |-crit|p2a: Pelipper          ← optional; present iff PS rolled a crit
//! |-damage|p2a: Pelipper|34/100
//! ```
//!
//! The `-crit` marker precedes the `-damage`. Both target the same
//! slot. The engine's draw site for crit is `Rng::crit()`, called
//! once per per-target damage roll inside `battle.rs::resolve_move`.
//! Spread moves call `crit()` once per target, so we emit one event
//! per damaging hit.

use crate::event::{Event, PokeSlot};
use crate::replay::{Replay, TurnView};
use vgc_engine_core::rng::RngEvent;

/// Walk one turn's events and emit one `RngEvent::Crit(bool)` per
/// damaging hit, in the same order the engine will draw them.
///
/// Algorithm: scan linearly. For every `|move|` event that is followed
/// by one or more `|-damage|` events (move-source: `from` is `None`),
/// emit `Crit(true)` if a `|-crit|` event for the same slot appeared
/// after the move and before the damage line, else `Crit(false)`.
///
/// Indirect-damage events (`from: Some("psn")`, recoil, weather, item
/// residuals, Rough Skin, etc.) are skipped — the engine doesn't draw
/// crit for those.
pub fn build_crit_oracle_for_turn(tv: &TurnView<'_>) -> Vec<RngEvent> {
    let mut out = Vec::new();
    let evs = tv.events;
    let mut i = 0;
    while i < evs.len() {
        if matches!(evs[i], Event::Move { .. }) {
            // From this `|move|` line, scan forward to the next `|move|`
            // (or end of turn). Within that window, every `|-damage|`
            // with `from: None` is a hit from this move's resolution.
            // Each damage is preceded by an optional `|-crit|` keyed to
            // the same target slot.
            i += 1;
            while i < evs.len() && !matches!(evs[i], Event::Move { .. }) {
                if let Event::Damage { slot, from: None, .. } = &evs[i] {
                    // Look back from this damage line within the current
                    // resolution window for a matching `-crit` slot.
                    let crit_marker = find_crit_for_slot_in_window(evs, i, slot);
                    out.push(RngEvent::Crit(crit_marker));
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Scan backwards from `damage_idx` until the previous `|move|` (or the
/// start of the slice). Returns true iff a `|-crit|` event for the
/// given slot appears in that window.
fn find_crit_for_slot_in_window(evs: &[Event], damage_idx: usize, target: &PokeSlot) -> bool {
    let mut j = damage_idx;
    while j > 0 {
        j -= 1;
        match &evs[j] {
            Event::Move { .. } => return false,
            Event::Crit(slot) if slot.player == target.player && slot.slot == target.slot => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Concatenate crit events across every turn of a replay, preserving
/// the per-turn ordering. Useful for the single-pass harness which
/// builds one queue at battle-start (the engine consumes them in
/// engine-resolution order, which matches PS's logged order for the
/// crit channel).
pub fn build_crit_oracle_for_replay(replay: &Replay) -> Vec<RngEvent> {
    let mut out = Vec::new();
    for tv in replay.turns() {
        if tv.number == 0 {
            continue; // pre-battle / team-preview
        }
        out.extend(build_crit_oracle_for_turn(&tv));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(player: u8, slot: char, nick: &str) -> PokeSlot {
        PokeSlot { player, slot, nickname: nick.to_string() }
    }

    fn mk_move(player: u8, slot_char: char, nick: &str, target: Option<PokeSlot>) -> Event {
        Event::Move {
            user: slot(player, slot_char, nick),
            move_name: "Tackle".into(),
            target,
        }
    }

    fn mk_damage(player: u8, slot_char: char, nick: &str, from: Option<&str>) -> Event {
        Event::Damage {
            slot: slot(player, slot_char, nick),
            hp: "50/100".into(),
            from: from.map(|s| s.into()),
        }
    }

    #[test]
    fn single_non_crit_hit_yields_one_crit_false() {
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            mk_damage(2, 'a', "Peli", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(build_crit_oracle_for_turn(&tv), vec![RngEvent::Crit(false)]);
    }

    #[test]
    fn crit_marker_before_damage_yields_crit_true() {
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            Event::Crit(slot(2, 'a', "Peli")),
            mk_damage(2, 'a', "Peli", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(build_crit_oracle_for_turn(&tv), vec![RngEvent::Crit(true)]);
    }

    #[test]
    fn spread_move_emits_one_crit_per_target_in_order() {
        // Rock Slide hits both p2 targets; one crits, the other doesn't.
        let evs = vec![
            mk_move(1, 'a', "Garchomp", None),
            Event::Crit(slot(2, 'a', "Peli")),
            mk_damage(2, 'a', "Peli", None),
            mk_damage(2, 'b', "Pika", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(
            build_crit_oracle_for_turn(&tv),
            vec![RngEvent::Crit(true), RngEvent::Crit(false)],
        );
    }

    #[test]
    fn indirect_damage_is_skipped() {
        // Burn DOT, Life Orb recoil, Rough Skin, weather damage — all
        // carry a `from:` tag and aren't drawn from `Rng::crit()`. They
        // must NOT produce Crit events.
        let evs = vec![
            mk_damage(1, 'a', "Chomp", Some("Life Orb")),
            mk_damage(2, 'a', "Peli", Some("brn")),
            mk_damage(1, 'b', "Pika", Some("Sandstorm")),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert!(build_crit_oracle_for_turn(&tv).is_empty());
    }

    #[test]
    fn two_moves_in_one_turn_yield_independent_crits() {
        // Move 1: crit; Move 2: no crit. Their crit windows don't bleed
        // into each other.
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            Event::Crit(slot(2, 'a', "Peli")),
            mk_damage(2, 'a', "Peli", None),
            mk_move(2, 'a', "Peli", Some(slot(1, 'a', "Chomp"))),
            mk_damage(1, 'a', "Chomp", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(
            build_crit_oracle_for_turn(&tv),
            vec![RngEvent::Crit(true), RngEvent::Crit(false)],
        );
    }

    #[test]
    fn fixture_has_exactly_one_crit() {
        // The bundled sample fixture has one `-crit` line (turn 2,
        // Acrobatics from Talonflame critting Sneasler). The full
        // sequence of damaging hits across the battle includes that
        // one true and all other false outcomes — exact count is
        // brittle here (counting damaging hits in a 6-turn doubles
        // game), but we can assert at least one Crit(true) and that
        // it sits before the bulk of the queue.
        let raw = include_str!("../tests/fixtures/sample.json");
        let replay = crate::Replay::from_json(raw).expect("parse fixture");
        let events = build_crit_oracle_for_replay(&replay);
        let trues = events.iter().filter(|e| matches!(e, RngEvent::Crit(true))).count();
        let falses = events.iter().filter(|e| matches!(e, RngEvent::Crit(false))).count();
        assert_eq!(trues, 1, "fixture has exactly 1 crit (the Acrobatics)");
        assert!(falses >= 8, "expected many non-crit damaging hits, got {falses}");
    }

    #[test]
    fn crit_for_wrong_slot_is_not_attributed() {
        // PS sometimes emits a -crit immediately before a damage event
        // that targets a DIFFERENT slot (rare, but possible in
        // multi-hit interleavings). Only same-slot markers count.
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'b', "Pika"))),
            Event::Crit(slot(2, 'a', "Peli")),
            mk_damage(2, 'b', "Pika", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(build_crit_oracle_for_turn(&tv), vec![RngEvent::Crit(false)]);
    }
}
