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

/// Walk one turn's events and emit one `RngEvent::PercentRoll(v)` per
/// `|move|` line whose accuracy outcome we can read off the log:
///
///   * `-miss` anywhere in the move's resolution window → `100`
///     (force-miss for any sub-100-acc move at the engine's
///     `roll > eff_acc` gate; harmless for 100-acc moves which PS
///     wouldn't have miss-marked anyway).
///   * otherwise, a `-damage` / `-heal` / `-boost` / `-status` /
///     `-unboost` for a target in the window → `1` (force-hit;
///     `1 <= eff_acc` for any non-zero accuracy).
///   * no observable effect (Protect, Wide Guard, immunity, Fail) →
///     skip. We can't tell if PS rolled accuracy for those, so the
///     engine falls back to splitmix and the queue position stays
///     coherent for later moves.
///
/// Spread moves: emit ONE event per `|move|` line. The engine rolls
/// accuracy per target, so only the first target's accuracy is
/// oracle-driven and remaining targets fall through to splitmix. That
/// is a strict improvement over "no oracle" without risking queue
/// desync.
///
/// Status moves with `accuracy == 255` (Protect, Tailwind, Trick Room,
/// etc.) never roll in the engine, so an emitted PercentRoll for them
/// would stay un-popped and corrupt later draws. We approximate "never
/// rolled" by skipping any move where neither `-miss` nor a
/// hit-effect is observed (covers status moves that simply succeed
/// without a per-target event, e.g. Tailwind → `-sidestart`).
pub fn build_accuracy_oracle_for_turn(tv: &TurnView<'_>) -> Vec<RngEvent> {
    let mut out = Vec::new();
    let evs = tv.events;
    let mut i = 0;
    while i < evs.len() {
        let Event::Move { user, target, .. } = &evs[i] else {
            i += 1;
            continue;
        };
        // Scan the resolution window [i+1 .. next move or end).
        let mut j = i + 1;
        let mut saw_miss = false;
        let mut saw_hit_effect = false;
        while j < evs.len() && !matches!(evs[j], Event::Move { .. }) {
            match &evs[j] {
                Event::Miss { source, .. } => {
                    // Only count miss if it belongs to this move's user.
                    if source.player == user.player && source.slot == user.slot {
                        saw_miss = true;
                    }
                }
                Event::Damage { from: None, slot, .. } => {
                    // A bare damage to anyone not the user is a hit on a target.
                    if !(slot.player == user.player && slot.slot == user.slot) {
                        saw_hit_effect = true;
                    }
                }
                Event::Status { slot, .. }
                | Event::Boost { slot, .. }
                | Event::Unboost { slot, .. }
                    if !(slot.player == user.player && slot.slot == user.slot) =>
                {
                    // Effects on a target (not the user) mean the move connected.
                    saw_hit_effect = true;
                }
                _ => {}
            }
            j += 1;
        }
        // Decision rules:
        //   * pure-hit (≥1 hit effect, no miss) → PercentRoll(1)
        //   * pure-miss (≥1 miss, no hit effect) → PercentRoll(100)
        //   * mixed (spread move where some targets hit, some missed) →
        //     SKIP. Emitting a single force-hit-or-miss would force the
        //     engine's first per-target roll the wrong way for ~half the
        //     remaining targets — strictly worse than letting splitmix
        //     guess. The engine's per-target rolls fall back to splitmix
        //     and the queue stays coherent for later moves.
        //   * no observable effect (Protect, Wide Guard, Fail, status
        //     moves with no trace) → skip. We can't tell whether PS
        //     rolled accuracy.
        if saw_miss && !saw_hit_effect {
            out.push(RngEvent::PercentRoll(100));
        } else if saw_hit_effect && !saw_miss && target.is_some() {
            out.push(RngEvent::PercentRoll(1));
        }
        i = j;
    }
    out
}

/// Concatenate accuracy events across every turn of a replay.
pub fn build_accuracy_oracle_for_replay(replay: &Replay) -> Vec<RngEvent> {
    let mut out = Vec::new();
    for tv in replay.turns() {
        if tv.number == 0 {
            continue;
        }
        out.extend(build_accuracy_oracle_for_turn(&tv));
    }
    out
}

/// Combined oracle: crit + accuracy events. The engine pops by variant
/// (Crit vs PercentRoll), so we can simply concatenate the two lists —
/// their per-variant relative order is preserved.
pub fn build_oracle_for_replay(replay: &Replay) -> Vec<RngEvent> {
    let mut out = build_crit_oracle_for_replay(replay);
    out.extend(build_accuracy_oracle_for_replay(replay));
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
    fn accuracy_hit_yields_percent_1() {
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            mk_damage(2, 'a', "Peli", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(
            build_accuracy_oracle_for_turn(&tv),
            vec![RngEvent::PercentRoll(1)],
        );
    }

    #[test]
    fn accuracy_miss_yields_percent_100() {
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            Event::Miss {
                source: slot(1, 'a', "Chomp"),
                target: Some(slot(2, 'a', "Peli")),
            },
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(
            build_accuracy_oracle_for_turn(&tv),
            vec![RngEvent::PercentRoll(100)],
        );
    }

    #[test]
    fn accuracy_protect_emits_nothing() {
        // Move was protected — no -miss, no -damage, no -boost on a target.
        // The engine might still roll accuracy, but we can't classify it.
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            // Just a -singleturn/-activate which we don't model — appears
            // as nothing to the accuracy walker.
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert!(build_accuracy_oracle_for_turn(&tv).is_empty());
    }

    #[test]
    fn accuracy_self_targeted_move_emits_nothing() {
        // Tailwind / Trick Room / Dragon Dance — no real target, no
        // accuracy gate in the engine. Skip.
        let evs = vec![
            mk_move(1, 'a', "Chomp", None),
            Event::Boost {
                slot: slot(1, 'a', "Chomp"),
                stat: "atk".into(),
                amount: 1,
            },
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert!(build_accuracy_oracle_for_turn(&tv).is_empty());
    }

    #[test]
    fn accuracy_two_moves_yield_two_events() {
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            mk_damage(2, 'a', "Peli", None),
            mk_move(2, 'a', "Peli", Some(slot(1, 'a', "Chomp"))),
            Event::Miss {
                source: slot(2, 'a', "Peli"),
                target: Some(slot(1, 'a', "Chomp")),
            },
        ];
        let tv = TurnView { number: 1, events: &evs };
        assert_eq!(
            build_accuracy_oracle_for_turn(&tv),
            vec![RngEvent::PercentRoll(1), RngEvent::PercentRoll(100)],
        );
    }

    #[test]
    fn combined_oracle_concatenates_crits_then_accuracy() {
        // Build a tiny synthetic replay-like input: 1 move, 1 crit, 1 hit.
        // Combined should produce [Crit(true), PercentRoll(1)].
        let evs = vec![
            mk_move(1, 'a', "Chomp", Some(slot(2, 'a', "Peli"))),
            Event::Crit(slot(2, 'a', "Peli")),
            mk_damage(2, 'a', "Peli", None),
        ];
        let tv = TurnView { number: 1, events: &evs };
        let mut combined = build_crit_oracle_for_turn(&tv);
        combined.extend(build_accuracy_oracle_for_turn(&tv));
        assert_eq!(
            combined,
            vec![RngEvent::Crit(true), RngEvent::PercentRoll(1)],
        );
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

// ---------------------------------------------------------------------------
// ps-rng-dump sidecar loader
// ---------------------------------------------------------------------------
//
// `tools/ps-rng-dump/dump.js` drives a Pokémon Showdown BattleStream
// under a fixed PRNG seed against an action sequence and writes a JSON
// dump with shape:
//
//   { "ok": true, "events": [
//       { "kind": "Crit", "value": true },
//       { "kind": "DamageRoll", "value": 7 },
//       { "kind": "PercentRoll", "value": true, "threshold": 30 },
//       ...
//   ] }
//
// `load_rng_dump` parses this into `Vec<RngEvent>` ready for
// `Rng::oracle_partial(events, fallback_seed)`. Variants not yet
// mapped to a vgc-engine draw site (`Chance` with arbitrary
// num/denom) are skipped — the engine falls back to Splitmix at those
// draw sites and we don't risk smashing a draw with the wrong type.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum DumpEvent {
    Crit { value: bool },
    DamageRoll { value: u8 },
    PercentRoll { value: bool, threshold: u8 },
    Range { value: u32, bound: u32 },
    Tiebreak { value: String },
    Chance {
        // arbitrary randomChance(num, denom) — no vgc-engine draw site
        // for these yet; skip on load. Fields are decoded so serde
        // doesn't choke on extra keys.
        #[allow(dead_code)] value: bool,
        #[allow(dead_code)] num: u32,
        #[allow(dead_code)] denom: u32,
    },
}

#[derive(Debug, Deserialize)]
struct Dump {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    events: Vec<DumpEvent>,
}

#[derive(Debug)]
pub enum DumpLoadError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotOk,
}

impl core::fmt::Display for DumpLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::NotOk => write!(f, "dump.ok == false"),
        }
    }
}
impl std::error::Error for DumpLoadError {}

/// Load a ps-rng-dump JSON file and lower it to the `Vec<RngEvent>`
/// queue consumed by `Rng::oracle_partial`. PercentRoll events
/// recover the percent value the engine sees from `(threshold, value)`:
/// PS asks `randomChance(threshold, 100)`, returning true when
/// `roll <= threshold` (PS uses 1..=100). We emit a percent that lands
/// just inside the matching half of the inclusive range:
///   * `value = true`  → emit the threshold itself (smallest pass)
///   * `value = false` → emit `threshold + 1` (smallest fail)
///
/// Range events with bound 16 are remapped to DamageRoll for
/// engine-side compatibility.
pub fn load_rng_dump(path: impl AsRef<std::path::Path>) -> Result<Vec<RngEvent>, DumpLoadError> {
    let bytes = std::fs::read(path).map_err(DumpLoadError::Io)?;
    let dump: Dump = serde_json::from_slice(&bytes).map_err(DumpLoadError::Json)?;
    if !dump.ok {
        return Err(DumpLoadError::NotOk);
    }
    let mut out = Vec::with_capacity(dump.events.len());
    for e in dump.events {
        match e {
            DumpEvent::Crit { value } => out.push(RngEvent::Crit(value)),
            DumpEvent::DamageRoll { value } => out.push(RngEvent::DamageRoll(value)),
            DumpEvent::PercentRoll { value, threshold } => {
                let v: u8 = if value {
                    threshold.clamp(1, 100)
                } else {
                    threshold.saturating_add(1).clamp(1, 100)
                };
                out.push(RngEvent::PercentRoll(v));
            }
            DumpEvent::Range { value, bound } => {
                if bound == 16 {
                    out.push(RngEvent::DamageRoll(value as u8));
                } else {
                    out.push(RngEvent::Range(value));
                }
            }
            DumpEvent::Tiebreak { value } => {
                // Parse "0xHEX" or decimal.
                let v = u64::from_str_radix(value.trim_start_matches("0x"), 16)
                    .or_else(|_| value.parse::<u64>())
                    .unwrap_or(0);
                out.push(RngEvent::Tiebreak(v));
            }
            DumpEvent::Chance { .. } => {
                // No vgc-engine draw site for arbitrary randomChance
                // yet — skip rather than corrupt the queue.
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod dump_tests {
    use super::*;

    #[test]
    fn load_fixture_pika_chomp_dump() {
        // Bundled fixture from tools/ps-rng-dump/fixture-pika-chomp.json:
        // 1-turn Pikachu Thunderbolt vs Garchomp Dragon Claw under seed
        // [1,2,3,4]. Expected events: accuracy (PercentRoll), crit,
        // damage roll, secondary chance (Chance, dropped on load).
        let raw = include_str!("../../../tools/ps-rng-dump/fixture-pika-chomp.json");
        let dump: Dump = serde_json::from_str(raw).unwrap();
        assert!(dump.ok);
        // Save to a temp file so load_rng_dump's file-read path is exercised.
        let tmp = std::env::temp_dir().join("vgc-engine-replay-dump-test.json");
        std::fs::write(&tmp, raw).unwrap();
        let events = load_rng_dump(&tmp).unwrap();
        // PercentRoll (accuracy) + Crit + DamageRoll. The Chance(3,10)
        // secondary-effect roll is dropped on purpose — vgc-engine
        // doesn't have a draw site for it yet.
        assert_eq!(events.len(), 3, "got: {events:?}");
        assert!(matches!(events[0], RngEvent::PercentRoll(_)));
        assert!(matches!(events[1], RngEvent::Crit(false)));
        assert!(matches!(events[2], RngEvent::DamageRoll(13)));
    }
}
