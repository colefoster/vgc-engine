//! Top-level [`Replay`] container: header metadata + parsed event stream,
//! built from a PS replay-JSON dump.

use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::parser::parse_line;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: u8,
    pub name: String,
    pub rating: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPreviewPoke {
    pub player: u8,
    pub details: String,
}

/// A borrowed slice of events belonging to a single turn. Turn 0 is a
/// synthetic bucket holding everything before the first `|turn|1` marker
/// (battle init: `start`, lead switches, weather kick-off from ability
/// triggers).
#[derive(Debug, Clone, Copy)]
pub struct TurnView<'a> {
    pub number: u32,
    pub events: &'a [Event],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    pub id: String,
    pub format: String,
    pub gametype: Option<String>,
    pub players: Vec<PlayerInfo>,
    pub team_preview: Vec<TeamPreviewPoke>,
    pub events: Vec<Event>,
    pub winner: Option<String>,
}

#[derive(Debug)]
pub enum ParseError {
    Json(serde_json::Error),
    MissingLog,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "replay json: {e}"),
            Self::MissingLog => write!(f, "replay json: missing 'log' field"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<serde_json::Error> for ParseError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl Replay {
    /// Parse a PS replay JSON dump (the shape returned by
    /// `https://replay.pokemonshowdown.com/<id>.json`).
    pub fn from_json(s: &str) -> Result<Self, ParseError> {
        let raw: serde_json::Value = serde_json::from_str(s)?;
        let log = raw
            .get("log")
            .and_then(|v| v.as_str())
            .ok_or(ParseError::MissingLog)?;

        let id = raw.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let format = raw.get("format").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let mut gametype = None;
        let mut players: Vec<PlayerInfo> = Vec::new();
        let mut team_preview: Vec<TeamPreviewPoke> = Vec::new();
        let mut events: Vec<Event> = Vec::new();
        let mut winner: Option<String> = None;

        for line in log.split('\n') {
            // Pull a few header fields directly from the raw line — they're
            // not modeled as `Event` variants because they're metadata, not
            // battle state.
            if let Some(rest) = line.strip_prefix("|gametype|") {
                gametype = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix("|player|") {
                let parts: Vec<&str> = rest.split('|').collect();
                if let Some(id_str) = parts.first()
                    && let Some(player_id) = id_str.strip_prefix('p').and_then(|s| s.parse().ok())
                {
                    let info = PlayerInfo {
                        id: player_id,
                        name: parts.get(1).map(|s| s.to_string()).unwrap_or_default(),
                        rating: parts.get(3).and_then(|s| s.parse().ok()),
                    };
                    // PS emits |player| once at room-join (full info) and again
                    // at battle start (often with empty name fields). Keep the
                    // first non-empty record per id.
                    if let Some(existing) = players.iter_mut().find(|p| p.id == player_id) {
                        if existing.name.is_empty() && !info.name.is_empty() {
                            *existing = info;
                        }
                    } else {
                        players.push(info);
                    }
                }
                continue;
            }

            let Some(ev) = parse_line(line) else { continue };

            match &ev {
                Event::Poke { player, details } => {
                    team_preview.push(TeamPreviewPoke {
                        player: *player,
                        details: details.clone(),
                    });
                }
                Event::Win(name) => winner = Some(name.clone()),
                _ => {}
            }
            events.push(ev);
        }

        Ok(Replay {
            id,
            format,
            gametype,
            players,
            team_preview,
            events,
            winner,
        })
    }

    /// Group `events` into per-turn slices. Each `|turn|N` marker starts
    /// a new bucket; everything before `|turn|1` is bucketed as turn 0
    /// (battle init: lead switches, weather kick-off from on-switch-in
    /// abilities). The trailing slice after the last `Turn` includes the
    /// `|win|` / `|tie|` events.
    ///
    /// The `Turn` event itself is dropped from each slice — slices are
    /// "events that happened during turn N," not "events including the
    /// turn marker."
    pub fn turns(&self) -> Vec<TurnView<'_>> {
        let mut out = Vec::new();
        let mut current_number: u32 = 0;
        let mut current_start: usize = 0;
        for (i, ev) in self.events.iter().enumerate() {
            if let Event::Turn(n) = ev {
                out.push(TurnView {
                    number: current_number,
                    events: &self.events[current_start..i],
                });
                current_number = *n;
                current_start = i + 1;
            }
        }
        out.push(TurnView {
            number: current_number,
            events: &self.events[current_start..],
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::PokeSlot;

    fn replay_with(events: Vec<Event>) -> Replay {
        Replay {
            id: String::new(),
            format: String::new(),
            gametype: None,
            players: Vec::new(),
            team_preview: Vec::new(),
            events,
            winner: None,
        }
    }

    fn fake_slot() -> PokeSlot {
        PokeSlot { player: 1, slot: 'a', nickname: "Foo".into() }
    }

    #[test]
    fn turns_bucket_pre_turn_events_as_zero() {
        let r = replay_with(vec![
            Event::Start,
            Event::Switch { slot: fake_slot(), details: "Foo, L50".into(), hp: "100/100".into() },
            Event::Turn(1),
            Event::Move {
                user: fake_slot(),
                move_name: "Tackle".into(),
                target: None,
            },
        ]);
        let turns = r.turns();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].number, 0);
        assert_eq!(turns[0].events.len(), 2); // Start + Switch
        assert_eq!(turns[1].number, 1);
        assert_eq!(turns[1].events.len(), 1); // Move
    }

    #[test]
    fn turns_drop_the_marker_itself() {
        let r = replay_with(vec![
            Event::Turn(1),
            Event::Upkeep,
            Event::Turn(2),
            Event::Upkeep,
        ]);
        let turns = r.turns();
        assert_eq!(turns.len(), 3);
        // turn 0 (empty preamble before any |turn| marker)
        assert_eq!(turns[0].number, 0);
        assert!(turns[0].events.is_empty());
        // turn 1 contains Upkeep, NOT the Turn(1) marker
        assert_eq!(turns[1].number, 1);
        assert_eq!(turns[1].events.len(), 1);
        assert!(matches!(turns[1].events[0], Event::Upkeep));
        // turn 2 contains the trailing Upkeep
        assert_eq!(turns[2].number, 2);
        assert!(matches!(turns[2].events[0], Event::Upkeep));
    }

    #[test]
    fn turns_trailing_win_goes_in_last_bucket() {
        let r = replay_with(vec![
            Event::Turn(1),
            Event::Faint(fake_slot()),
            Event::Win("p1".into()),
        ]);
        let turns = r.turns();
        assert_eq!(turns.last().unwrap().number, 1);
        assert_eq!(turns.last().unwrap().events.len(), 2);
        assert!(matches!(turns.last().unwrap().events[1], Event::Win(_)));
    }
}
