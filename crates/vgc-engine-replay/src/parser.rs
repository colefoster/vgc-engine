//! Line-level PS protocol parser.
//!
//! `parse_line` takes one log line (with or without the leading `|`) and
//! returns the matching [`Event`]. Lines that don't carry battle state
//! (chat `|c|`, timer `|t:|`, joins `|j|`/`|l|`, raw HTML, etc.) and any
//! message the modeled enum doesn't cover return [`Event::Other`].

use crate::event::{Event, PokeSlot};

pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.strip_prefix('|').unwrap_or(line);
    if line.is_empty() {
        return None;
    }
    let mut parts = line.split('|');
    let kind = parts.next()?;
    let rest: Vec<&str> = parts.collect();

    let kv = |key: &str| -> Option<String> {
        rest.iter()
            .find_map(|p| p.strip_prefix(&format!("[{key}] ")).map(str::to_string))
    };
    let from = || kv("from");
    let of = || kv("of").as_deref().and_then(parse_slot);

    Some(match kind {
        "turn" => Event::Turn(rest.first()?.parse().ok()?),
        "start" => Event::Start,
        "upkeep" => Event::Upkeep,
        "win" => Event::Win(rest.first()?.to_string()),
        "tie" => Event::Tie,
        "clearpoke" => Event::ClearPoke,
        "teampreview" => Event::TeamPreview(rest.first().and_then(|s| s.parse().ok())),
        "teamsize" => Event::TeamSize {
            player: parse_player(rest.first()?)?,
            size: rest.get(1)?.parse().ok()?,
        },
        "poke" => Event::Poke {
            player: parse_player(rest.first()?)?,
            details: rest.get(1)?.to_string(),
        },
        "move" => Event::Move {
            user: parse_slot(rest.first()?)?,
            move_name: rest.get(1)?.to_string(),
            target: rest.get(2).and_then(|s| parse_slot(s)),
        },
        "switch" => Event::Switch {
            slot: parse_slot(rest.first()?)?,
            details: rest.get(1)?.to_string(),
            hp: rest.get(2).map(|s| s.to_string()).unwrap_or_default(),
        },
        "drag" => Event::Drag {
            slot: parse_slot(rest.first()?)?,
            details: rest.get(1)?.to_string(),
            hp: rest.get(2).map(|s| s.to_string()).unwrap_or_default(),
        },
        "faint" => Event::Faint(parse_slot(rest.first()?)?),
        "-damage" => Event::Damage {
            slot: parse_slot(rest.first()?)?,
            hp: rest.get(1).map(|s| s.to_string()).unwrap_or_default(),
            from: from(),
        },
        "-heal" => Event::Heal {
            slot: parse_slot(rest.first()?)?,
            hp: rest.get(1).map(|s| s.to_string()).unwrap_or_default(),
            from: from(),
        },
        "-boost" => Event::Boost {
            slot: parse_slot(rest.first()?)?,
            stat: rest.get(1)?.to_string(),
            amount: rest.get(2)?.parse().ok()?,
        },
        "-unboost" => Event::Unboost {
            slot: parse_slot(rest.first()?)?,
            stat: rest.get(1)?.to_string(),
            amount: rest.get(2)?.parse().ok()?,
        },
        "-status" => Event::Status {
            slot: parse_slot(rest.first()?)?,
            status: rest.get(1)?.to_string(),
        },
        "-ability" => Event::Ability {
            slot: parse_slot(rest.first()?)?,
            ability: rest.get(1)?.to_string(),
            from: from(),
        },
        "-item" => Event::Item {
            slot: parse_slot(rest.first()?)?,
            item: rest.get(1)?.to_string(),
            from: from(),
        },
        "-enditem" => Event::EndItem {
            slot: parse_slot(rest.first()?)?,
            item: rest.get(1)?.to_string(),
            from: from(),
        },
        "-curestatus" => Event::CureStatus {
            slot: parse_slot(rest.first()?)?,
            status: rest.get(1)?.to_string(),
        },
        "-weather" => Event::Weather {
            weather: rest.first()?.to_string(),
            from: from(),
            of: of(),
        },
        "-fieldstart" => Event::FieldStart {
            effect: rest.first()?.to_string(),
            from: from(),
            of: of(),
        },
        "-fieldend" => Event::FieldEnd {
            effect: rest.first()?.to_string(),
        },
        "-sidestart" => Event::SideStart {
            side: rest.first()?.to_string(),
            effect: rest.get(1)?.to_string(),
        },
        "-sideend" => Event::SideEnd {
            side: rest.first()?.to_string(),
            effect: rest.get(1)?.to_string(),
        },
        _ => Event::Other(line.to_string()),
    })
}

/// `p1a: Sneasler` → `PokeSlot { 1, 'a', "Sneasler" }`. Also accepts `p2: ...`
/// (no slot letter) for singles / pre-switch references.
fn parse_slot(s: &str) -> Option<PokeSlot> {
    let (head, nick) = s.split_once(": ")?;
    let bytes = head.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'p' {
        return None;
    }
    let player = (bytes[1] as char).to_digit(10)? as u8;
    let slot = if bytes.len() >= 3 {
        bytes[2] as char
    } else {
        'a'
    };
    Some(PokeSlot {
        player,
        slot,
        nickname: nick.to_string(),
    })
}

fn parse_player(s: &str) -> Option<u8> {
    s.strip_prefix('p')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn() {
        assert_eq!(parse_line("|turn|3"), Some(Event::Turn(3)));
    }

    #[test]
    fn move_with_target() {
        let ev = parse_line("|move|p1a: Sneasler|Close Combat|p2a: Garchomp").unwrap();
        match ev {
            Event::Move { user, move_name, target } => {
                assert_eq!(user.player, 1);
                assert_eq!(user.slot, 'a');
                assert_eq!(user.nickname, "Sneasler");
                assert_eq!(move_name, "Close Combat");
                let t = target.unwrap();
                assert_eq!(t.player, 2);
                assert_eq!(t.slot, 'a');
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn weather_with_from() {
        let ev = parse_line("|-weather|RainDance|[from] ability: Drizzle|[of] p1b: Pelipper").unwrap();
        match ev {
            Event::Weather { weather, from, .. } => {
                assert_eq!(weather, "RainDance");
                assert_eq!(from.as_deref(), Some("ability: Drizzle"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn boost_signed() {
        let ev = parse_line("|-boost|p1b: Archaludon|def|1").unwrap();
        assert!(matches!(ev, Event::Boost { amount: 1, .. }));
    }

    #[test]
    fn faint() {
        let ev = parse_line("|faint|p2b: Talonflame").unwrap();
        assert!(matches!(ev, Event::Faint(s) if s.player == 2 && s.slot == 'b'));
    }

    #[test]
    fn win() {
        assert_eq!(
            parse_line("|win|TorkoalWQuickclaw"),
            Some(Event::Win("TorkoalWQuickclaw".to_string()))
        );
    }

    #[test]
    fn poke() {
        let ev = parse_line("|poke|p1|Pelipper, L50, F|").unwrap();
        match ev {
            Event::Poke { player, details } => {
                assert_eq!(player, 1);
                assert_eq!(details, "Pelipper, L50, F");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn unknown_is_other() {
        let ev = parse_line("|c|user|hello").unwrap();
        assert!(matches!(ev, Event::Other(_)));
    }

    #[test]
    fn empty_line_skipped() {
        assert!(parse_line("").is_none());
        assert!(parse_line("|").is_none());
    }
}
