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
}
