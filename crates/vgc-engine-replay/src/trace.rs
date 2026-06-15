//! HP traces — the comparable signal for the replay-differential
//! agreement scorer.
//!
//! Every PS log event that carries HP info (Damage / Heal / Switch /
//! Drag) is parsed into a uniform [`HpEvent`] containing slot, current-
//! HP fraction in `[0.0, 1.0]`, and a `fainted` flag. The Faint marker
//! itself emits an `HpEvent` with `fraction = 0.0, fainted = true` so
//! the scorer can detect the death even if the prior Damage event
//! already reported `0 fnt`.
//!
//! Output is intentionally lossy on max-HP — public-ladder replays
//! report percentages (`/100`) when ratings are hidden, so absolute HP
//! isn't recoverable from the log. Fraction comparison is the right
//! grain for the agreement gate anyway (`≥80% turn-level agreement` =
//! per-turn HP fractions within tolerance).

use crate::event::{Event, PokeSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpSource {
    /// `|-damage|...`
    Damage,
    /// `|-heal|...`
    Heal,
    /// `|switch|...` or `|drag|...` — useful to anchor HP on switch-in
    /// (Regenerator, mid-battle swap reveals).
    SwitchIn,
    /// `|faint|...` — emitted as a `fraction=0.0, fainted=true` event.
    Faint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HpEvent {
    pub slot: PokeSlot,
    /// `[0.0, 1.0]`. `0.0` when fainted.
    pub fraction: f32,
    pub fainted: bool,
    pub source: HpSource,
}

/// Walk an event slice and extract every HP-carrying event in order.
/// Unparseable HP strings are skipped silently — protocol forward-compat
/// is more important than crashing the scorer on a weird format edge.
pub fn hp_trace(events: &[Event]) -> Vec<HpEvent> {
    let mut out = Vec::new();
    for ev in events {
        match ev {
            Event::Damage { slot, hp, .. } => push_hp(&mut out, slot, hp, HpSource::Damage),
            Event::Heal { slot, hp, .. } => push_hp(&mut out, slot, hp, HpSource::Heal),
            Event::Switch { slot, hp, .. } | Event::Drag { slot, hp, .. } => {
                push_hp(&mut out, slot, hp, HpSource::SwitchIn)
            }
            Event::Faint(slot) => out.push(HpEvent {
                slot: slot.clone(),
                fraction: 0.0,
                fainted: true,
                source: HpSource::Faint,
            }),
            _ => {}
        }
    }
    out
}

fn push_hp(out: &mut Vec<HpEvent>, slot: &PokeSlot, hp_str: &str, source: HpSource) {
    if let Some((fraction, fainted)) = parse_hp(hp_str) {
        out.push(HpEvent {
            slot: slot.clone(),
            fraction,
            fainted,
            source,
        });
    }
}

/// Parse a PS HP string. Examples:
///
///   "100/100"      → (1.0, false)
///   "70/100"       → (0.70, false)
///   "70/100 par"   → (0.70, false)   (trailing status code ignored)
///   "0 fnt"        → (0.0, true)
///   ""             → None
pub fn parse_hp(s: &str) -> Option<(f32, bool)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // "0 fnt" / "fnt" — fainted, no fraction info.
    if s == "fnt" || s.starts_with("0 fnt") {
        return Some((0.0, true));
    }
    // Strip any trailing status code ("70/100 par" → "70/100").
    let core = s.split_whitespace().next()?;
    let (num, den) = core.split_once('/')?;
    let num: f32 = num.parse().ok()?;
    let den: f32 = den.parse().ok()?;
    if den <= 0.0 {
        return None;
    }
    Some(((num / den).clamp(0.0, 1.0), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(p: u8, c: char) -> PokeSlot {
        PokeSlot { player: p, slot: c, nickname: "x".into() }
    }

    #[test]
    fn hp_full() {
        assert_eq!(parse_hp("100/100"), Some((1.0, false)));
    }

    #[test]
    fn hp_partial() {
        let (f, fnt) = parse_hp("70/100").unwrap();
        assert!((f - 0.70).abs() < 1e-6);
        assert!(!fnt);
    }

    #[test]
    fn hp_with_status_code() {
        let (f, fnt) = parse_hp("45/100 par").unwrap();
        assert!((f - 0.45).abs() < 1e-6);
        assert!(!fnt);
    }

    #[test]
    fn hp_fainted() {
        assert_eq!(parse_hp("0 fnt"), Some((0.0, true)));
        assert_eq!(parse_hp("fnt"), Some((0.0, true)));
    }

    #[test]
    fn hp_empty_is_none() {
        assert_eq!(parse_hp(""), None);
        assert_eq!(parse_hp("garbage"), None);
    }

    #[test]
    fn trace_picks_damage_heal_switch_faint() {
        let events = vec![
            Event::Switch {
                slot: slot(1, 'a'),
                details: "Pelipper, L50, F".into(),
                hp: "100/100".into(),
            },
            Event::Damage {
                slot: slot(1, 'a'),
                hp: "45/100".into(),
                from: None,
            },
            Event::Heal {
                slot: slot(1, 'a'),
                hp: "70/100".into(),
                from: Some("item: Sitrus Berry".into()),
            },
            Event::Damage {
                slot: slot(1, 'a'),
                hp: "0 fnt".into(),
                from: None,
            },
            Event::Faint(slot(1, 'a')),
        ];
        let trace = hp_trace(&events);
        assert_eq!(trace.len(), 5);
        assert_eq!(trace[0].source, HpSource::SwitchIn);
        assert_eq!(trace[0].fraction, 1.0);
        assert_eq!(trace[1].source, HpSource::Damage);
        assert!((trace[1].fraction - 0.45).abs() < 1e-6);
        assert_eq!(trace[2].source, HpSource::Heal);
        assert!((trace[2].fraction - 0.70).abs() < 1e-6);
        assert_eq!(trace[3].source, HpSource::Damage);
        assert!(trace[3].fainted);
        assert_eq!(trace[4].source, HpSource::Faint);
        assert!(trace[4].fainted);
    }

    #[test]
    fn trace_skips_non_hp_events() {
        let events = vec![
            Event::Turn(1),
            Event::Move {
                user: slot(1, 'a'),
                move_name: "Tackle".into(),
                target: None,
            },
            Event::Upkeep,
        ];
        assert!(hp_trace(&events).is_empty());
    }
}
