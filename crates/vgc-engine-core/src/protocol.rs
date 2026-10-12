//! Opt-in Pokémon Showdown battle-protocol log.
//!
//! The log stores exactly what PS keeps in `battle.log` (sim/battle.ts:3083
//! `addSplit`, :3091 `add`): one string per line, with a private/public pair
//! written as three entries — `|split|pN`, the secret line (exact HP, seen by
//! side N and the omniscient channel), the public line (percentage HP, seen
//! by everyone else; empty = hidden). [`ProtocolLog::view`] renders a channel
//! the way PS `extractChannelMessages` (sim/battle.ts:35) does.
//!
//! Logging is off unless a battle is built with [`ProtocolOptions`]. The
//! battle holds the log behind a [`ProtoSlot`]; every emission site checks
//! [`ProtoSlot::on`] before formatting anything, so the hot loop pays one
//! branch per site and allocates nothing when it is off. Clones of a battle
//! (solver / search) drop the log, and serde skips it, so snapshots are
//! unchanged.

use std::fmt;

/// How lines are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolMode {
    /// Byte-exact PS protocol lines (the conformance target).
    Ps,
    /// The same events, minus purely cosmetic lines: `|t:|` timestamps,
    /// blank `|` separators, `|j|`, `|gametype|`, `|player|`, `|gen|`,
    /// `|tier|`, `|rule|`, `|uhtml|` / `|uhtmlchange|` and `|-hint|`.
    Compact,
}

impl ProtocolMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ps" => Some(Self::Ps),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

/// Which channel to render (PS `extractChannelMessages` channel ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolView {
    /// Channel -1: every secret line (exact HP for both sides).
    Omniscient,
    /// Channel 0: public lines only.
    Spectator,
    /// Channel 1: p1's secrets, p2's public lines.
    P1,
    /// Channel 2: p2's secrets, p1's public lines.
    P2,
}

impl ProtocolView {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "omniscient" | "-1" | "all" => Some(Self::Omniscient),
            "spectator" | "0" => Some(Self::Spectator),
            "p1" | "1" => Some(Self::P1),
            "p2" | "2" => Some(Self::P2),
            _ => None,
        }
    }
}

/// Construction-time settings for a logged battle.
#[derive(Debug, Clone)]
pub struct ProtocolOptions {
    pub mode: ProtocolMode,
    /// PS `side.name` (`|player|`, `|win|`). PS defaults: "Player 1/2".
    pub player_names: [String; 2],
    /// Per-side nicknames by team index; `None` = PS's default name, the
    /// base species (sim/pokemon.ts:328-332).
    pub nicknames: [Vec<Option<String>>; 2],
    /// Fixed `|t:|` value; `None` = wall-clock seconds (PS
    /// `Math.floor(Date.now() / 1000)`).
    pub timestamp: Option<u64>,
    /// The PS format id, for the preamble (`|tier|`, `|rule|`, team
    /// preview). `None` falls back to the battle's own format id.
    pub format_id: Option<String>,
}

impl Default for ProtocolOptions {
    fn default() -> Self {
        Self {
            mode: ProtocolMode::Ps,
            player_names: ["Player 1".to_string(), "Player 2".to_string()],
            nicknames: [Vec::new(), Vec::new()],
            timestamp: None,
            format_id: None,
        }
    }
}

impl ProtocolOptions {
    pub fn new(mode: ProtocolMode) -> Self {
        Self { mode, ..Self::default() }
    }
}

/// The preamble PS prints for a format: `|tier|` name, `|rule|` lines (in
/// rule-table order), and its team-preview rules.
#[derive(Debug, Clone, Copy)]
pub struct FormatPreamble {
    pub tier: &'static str,
    pub rules: &'static [&'static str],
    /// Team Preview (data/rulesets.ts:633): `|clearpoke|`, `|poke|`,
    /// `|teampreview|`.
    pub team_preview: bool,
    /// Open Team Sheets (data/rulesets.ts:1990-2000): the `otsrequest`
    /// uhtml at preview, cleared at start.
    pub open_team_sheets: bool,
    /// `pickedTeamSize` (`|teampreview|N`); 0 = whole team.
    pub picked_team_size: u8,
}

const CHAMPIONS_RULES: &[&str] = &[
    "Species Clause: Limit one of each Pokémon",
    "Item Clause: Limit 1 of each item",
];

/// PS config/formats.ts entries the engine knows. Unknown ids get a generic
/// gen 9 custom-game preamble.
pub fn format_preamble(format_id: &str, doubles: bool) -> FormatPreamble {
    let id = format_id.to_ascii_lowercase();
    match id.as_str() {
        "gen9championsvgc2026regmc" | "regmc" => FormatPreamble {
            tier: "[Gen 9 Champions] VGC 2026 Reg M-C",
            rules: CHAMPIONS_RULES,
            team_preview: true,
            open_team_sheets: true,
            picked_team_size: 4,
        },
        "gen9championsvgc2026regmb" | "regmb" => FormatPreamble {
            tier: "[Gen 9 Champions] VGC 2026 Reg M-B",
            rules: CHAMPIONS_RULES,
            team_preview: true,
            open_team_sheets: true,
            picked_team_size: 4,
        },
        _ => FormatPreamble {
            tier: if doubles { "[Gen 9] Doubles Custom Game" } else { "[Gen 9] Custom Game" },
            rules: &[],
            team_preview: false,
            open_team_sheets: false,
            picked_team_size: 0,
        },
    }
}

/// The battle's handle on its log. `Clone` yields an empty slot: solver and
/// search clones never carry (or copy) the log.
#[derive(Default)]
pub struct ProtoSlot(pub(crate) Option<Box<ProtocolLog>>);

impl Clone for ProtoSlot {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl fmt::Debug for ProtoSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => f.write_str("ProtoSlot(off)"),
            Some(l) => write!(f, "ProtoSlot({} lines)", l.lines.len()),
        }
    }
}

impl ProtoSlot {
    /// True when the battle is logging and the log is still open (PS stops
    /// adding lines once the battle has ended).
    #[inline(always)]
    pub fn on(&self) -> bool {
        match &self.0 {
            Some(l) => !l.ended,
            None => false,
        }
    }
    #[inline]
    pub fn log(&self) -> Option<&ProtocolLog> {
        self.0.as_deref()
    }
    #[inline]
    pub fn log_mut(&mut self) -> Option<&mut ProtocolLog> {
        self.0.as_deref_mut()
    }
    pub(crate) fn add(&mut self, line: String) {
        if let Some(l) = self.0.as_deref_mut() {
            if !l.ended {
                l.lines.push(line);
            }
        }
    }
}

/// The log itself.
#[derive(Debug, Clone)]
pub struct ProtocolLog {
    pub mode: ProtocolMode,
    pub(crate) lines: Vec<String>,
    /// PS `lastMoveLine` (sim/battle.ts:3117): the `|move|` line
    /// `attrLastMove` / `retargetLastMove` edit.
    pub(crate) last_move_line: Option<usize>,
    /// Set by `|win|` / `|tie|`: PS adds nothing after `battle.end`.
    pub(crate) ended: bool,
    /// `[side][team index]` PS `pokemon.name`.
    pub(crate) names: [Vec<String>; 2],
    /// `[side][team index]` species shown in `details` (PS
    /// `pokemon.details`, changed only by a permanent forme change).
    pub(crate) details_species: [Vec<u16>; 2],
    pub(crate) player_names: [String; 2],
    pub(crate) timestamp: Option<u64>,
    /// PS `faintQueue`: mons whose HP hit 0, in order, awaiting `|faint|`.
    pub(crate) faint_queue: Vec<(u8, u8)>,
    /// Bit `side * 8 + team index`: `|faint|` already printed.
    pub(crate) fainted_logged: u16,
    /// Champions HP display (sim/pokemon.ts:2067-2074).
    pub(crate) champions_hp: bool,
    /// Open Team Sheets: clear the `otsrequest` box at start.
    pub(crate) ots: bool,
}

impl ProtocolLog {
    pub(crate) fn new(opts: &ProtocolOptions) -> Self {
        Self {
            mode: opts.mode,
            lines: Vec::with_capacity(256),
            last_move_line: None,
            ended: false,
            names: [Vec::new(), Vec::new()],
            details_species: [Vec::new(), Vec::new()],
            player_names: opts.player_names.clone(),
            timestamp: opts.timestamp,
            faint_queue: Vec::new(),
            fainted_logged: 0,
            champions_hp: false,
            ots: false,
        }
    }

    /// The raw PS `battle.log` (with `|split|` markers), as PS stores it.
    pub fn raw_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// The `|t:|` value.
    pub(crate) fn now(&self) -> u64 {
        self.timestamp.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
    }

    pub(crate) fn push(&mut self, line: String) {
        if !self.ended {
            self.lines.push(line);
        }
    }

    /// PS `addSplit(side, secret, shared)`: `shared = None` hides the line
    /// from everyone but `side` (and the omniscient channel).
    pub(crate) fn push_split(&mut self, side: u8, secret: String, shared: Option<String>) {
        if self.ended {
            return;
        }
        self.lines.push(format!("|split|p{}", side + 1));
        self.lines.push(secret);
        self.lines.push(shared.unwrap_or_default());
    }

    pub(crate) fn push_move(&mut self, line: String) {
        if self.ended {
            return;
        }
        self.last_move_line = Some(self.lines.len());
        self.lines.push(line);
    }

    /// PS `attrLastMove` (sim/battle.ts:3122-3140).
    pub(crate) fn attr_last_move(&mut self, attr: &str) {
        let Some(i) = self.last_move_line else { return };
        if self.ended || i >= self.lines.len() {
            return;
        }
        if self.lines[i].starts_with("|-anim|") {
            if attr.split('|').any(|a| a == "[still]") {
                self.lines.remove(i);
                self.last_move_line = None;
                return;
            }
        } else if attr.split('|').any(|a| a == "[still]") {
            // "If no animation plays, the target should never be known".
            let mut parts: Vec<&str> = self.lines[i].split('|').collect();
            if parts.len() > 4 {
                parts[4] = "";
            }
            self.lines[i] = parts.join("|");
        }
        self.lines[i].push('|');
        self.lines[i].push_str(attr);
    }

    /// PS `retargetLastMove` (sim/battle.ts:3142-3147).
    pub(crate) fn retarget_last_move(&mut self, target: &str) {
        let Some(i) = self.last_move_line else { return };
        if self.ended || i >= self.lines.len() {
            return;
        }
        let mut parts: Vec<String> = self.lines[i].split('|').map(str::to_string).collect();
        if parts.len() > 4 {
            parts[4] = target.to_string();
        }
        self.lines[i] = parts.join("|");
    }

    /// Render one channel, PS `extractChannelMessages` (sim/battle.ts:35-59).
    /// `Compact` mode drops the cosmetic lines here.
    pub fn view(&self, view: ProtocolView) -> Vec<String> {
        let channel: i8 = match view {
            ProtocolView::Omniscient => -1,
            ProtocolView::Spectator => 0,
            ProtocolView::P1 => 1,
            ProtocolView::P2 => 2,
        };
        let mut out = Vec::with_capacity(self.lines.len());
        let mut i = 0;
        while i < self.lines.len() {
            let l = &self.lines[i];
            if let Some(p) = l.strip_prefix("|split|p") {
                let player: i8 = p.parse().unwrap_or(0);
                let secret = self.lines.get(i + 1).map(String::as_str).unwrap_or("");
                let shared = self.lines.get(i + 2).map(String::as_str).unwrap_or("");
                let line = if channel == -1 || player == channel { secret } else { shared };
                if !line.is_empty() {
                    out.push(line.to_string());
                }
                i += 3;
                continue;
            }
            // `.+` in PS's split regex: empty entries never make a line.
            if !l.is_empty() {
                out.push(l.clone());
            }
            i += 1;
        }
        if self.mode == ProtocolMode::Compact {
            out.retain(|l| !is_cosmetic(l));
        }
        out
    }
}

/// Lines `Compact` mode drops: no game information.
pub fn is_cosmetic(line: &str) -> bool {
    if line == "|" {
        return true;
    }
    let tag = line.split('|').nth(1).unwrap_or("");
    matches!(
        tag,
        "t:" | "j" | "J" | "gametype" | "player" | "gen" | "tier" | "rule" | "uhtml" | "uhtmlchange" | "-hint" | "rated"
    )
}

/// PS `Pokemon.getHealth` (sim/pokemon.ts:2060-2104), split into the
/// secret (exact) and shared (percentage) strings.
pub(crate) fn health(hp: u16, maxhp: u16, status: &str, champions: bool) -> (String, String) {
    if hp == 0 {
        return ("0 fnt".to_string(), "0 fnt".to_string());
    }
    let mut secret = format!("{hp}/{maxhp}");
    let (hp32, max32) = (hp as u32, maxhp.max(1) as u32);
    let mut shared = if champions {
        // Champions: floor, at least 1; 20 / 50 carry the colour band.
        let pct = (100 * hp32 / max32).max(1);
        let mut s = format!("{pct}/100");
        if pct == 20 {
            s.push(if hp32 * 5 > max32 { 'y' } else { 'r' });
        } else if pct == 50 {
            s.push(if hp32 * 2 > max32 { 'g' } else { 'y' });
        }
        s
    } else {
        // Gen 7+: ceil, 99 for a scratched full bar.
        let mut pct = (100 * hp32).div_ceil(max32);
        if pct == 100 && hp32 < max32 {
            pct = 99;
        }
        format!("{pct}/100")
    };
    if !status.is_empty() {
        secret.push(' ');
        secret.push_str(status);
        shared.push(' ');
        shared.push_str(status);
    }
    (secret, shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn champions_health_floors_with_colour_bands() {
        assert_eq!(health(141, 191, "", true).1, "73/100");
        assert_eq!(health(1, 300, "", true).1, "1/100");
        // 20%: yellow above a fifth, red at or below.
        assert_eq!(health(41, 200, "", true).1, "20/100y");
        assert_eq!(health(40, 200, "", true).1, "20/100r");
        assert_eq!(health(101, 200, "par", true), ("101/200 par".into(), "50/100g par".into()));
        assert_eq!(health(0, 200, "", true).0, "0 fnt");
    }

    #[test]
    fn standard_health_ceils_and_never_shows_full_when_scratched() {
        assert_eq!(health(141, 191, "", false).1, "74/100");
        assert_eq!(health(399, 400, "", false).1, "99/100");
    }

    #[test]
    fn views_follow_extract_channel_messages() {
        let mut l = ProtocolLog::new(&ProtocolOptions::default());
        l.push("|turn|1".into());
        l.push_split(0, "|-damage|p1a: A|50/100".into(), Some("|-damage|p1a: A|50/100".into()));
        l.push_split(1, "|-heal|p2a: B|9/10".into(), None);
        l.push(String::new());
        assert_eq!(l.view(ProtocolView::Spectator), vec!["|turn|1", "|-damage|p1a: A|50/100"]);
        assert_eq!(l.view(ProtocolView::P2), vec!["|turn|1", "|-damage|p1a: A|50/100", "|-heal|p2a: B|9/10"]);
        assert_eq!(l.view(ProtocolView::Omniscient).len(), 3);
    }

    #[test]
    fn attr_last_move_still_blanks_the_target() {
        let mut l = ProtocolLog::new(&ProtocolOptions::default());
        l.push_move("|move|p1b: Gallade|Protect|p1b: Gallade".into());
        l.attr_last_move("[still]");
        assert_eq!(l.lines[0], "|move|p1b: Gallade|Protect||[still]");
    }
}
