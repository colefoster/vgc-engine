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

/// The effect behind a boost, which picks its protocol lines (PS
/// `battle.boost`, sim/battle.ts:2048-2081).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BoostFx {
    /// A move's own boost (`effectType === 'Move'`): a zero change prints.
    #[default]
    Move,
    /// A move secondary / self effect (`isSecondary` / `isSelf`): a zero
    /// change prints nothing.
    MoveSecondary,
    /// `[from] item: X`.
    Item(u16),
    /// `-ability|X|boost` first (unless `isSecondary`), then the lines.
    Ability(u16),
    /// The ability already announced itself (`isSecondary`).
    AbilitySecondary(u16),
    /// `isSelf` ability boost (Competitive, Defiant): zero changes print.
    AbilitySelf(u16),
    /// No lines.
    Silent,
}

/// The source effect of a status infliction, which picks its `-status`
/// suffix (data/conditions.ts brn / par / slp ... onStart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusFx {
    /// A move or a condition: plain (slp prints `[from] move:` for moves).
    #[default]
    Plain,
    /// `[from] move: X` for sleep.
    Move(u16),
    /// `[from] ability: X|[of] <source side, slot>`.
    Ability(u16, u8, u8),
    /// `[from] item: Flame Orb` / `Toxic Orb`.
    Item(u16),
    /// Nothing printed.
    Silent,
    /// Plain even for sleep: a secondary's onHit trySetStatus, whose
    /// effect is not the move (data/moves.ts direclaw).
    Bare,
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
            l.push(line);
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
    /// Window key when each `faint_queue` entry was queued: a spread move's
    /// targets faint in target order, not the engine's processing order.
    pub(crate) faint_keys: Vec<u32>,
    /// Bit `side * 8 + team index`: `|faint|` already printed.
    pub(crate) fainted_logged: u16,
    /// Champions HP display (sim/pokemon.ts:2067-2074).
    pub(crate) champions_hp: bool,
    /// Open Team Sheets: clear the `otsrequest` box at start.
    pub(crate) ots: bool,
    /// The next `do_switch`'s line tag and suffix (`drag`, `|[from] U-turn`).
    pub(crate) switch_ctx: Option<(&'static str, String)>,
    /// `[side][slot]` the move each active last printed a `|move|` for.
    pub(crate) last_move: [[u16; 2]; 2],
    /// A defrost move thawed its user: print the cure after `|move|`.
    pub(crate) defrost_pending: bool,
    /// The next `apply_boosts` call's effect.
    pub(crate) boost_fx: BoostFx,
    /// The next status infliction's effect.
    pub(crate) status_fx: StatusFx,
    /// The move being executed (PS `activeMove`), `u16::MAX` between actions.
    pub(crate) active_move: u16,
    /// A `|move|` line not yet written: the BeforeMove-style checks the
    /// engine runs after the move's announcement point can still cancel
    /// it (`cant` lines precede `|move|` in PS).
    pub(crate) pending_move: Option<String>,
    /// An open reorder window (a spread move's hit steps).
    pub(crate) window: Option<Box<Window>>,
    /// The crit flag of the hit being resolved (multi-hit re-rolls).
    pub(crate) hit_crit: bool,
    /// `-hitcount` to print after the hit loop: (side, slot, hits).
    pub(crate) hitcount: Option<(u8, u8, u8)>,
    /// The hit loop's end (faintMessages, `-hitcount`) already printed.
    pub(crate) loop_ended: bool,
    /// Lines printed right before a mon's `|faint|` (Destiny Bond).
    pub(crate) faint_prefix: Vec<(u8, u8, String)>,
    /// Magic Room's setter, named again at its end.
    pub(crate) magic_room_source: String,
    /// The status move being resolved: (user side, user slot, target).
    pub(crate) status_ctx: Option<(u8, u8, Option<(u8, u8)>)>,
    /// Lines written since the current `|move|` was announced.
    pub(crate) since_move: u32,
    /// Residual window: each active's speed key (`[side][slot]`).
    pub(crate) res_speed: [[u8; 2]; 2],
    /// Scratch HP for a residual handler.
    pub(crate) res_hp_before: u16,
    /// `[side][team index]` the partial-trapping move holding the mon.
    pub(crate) trap_move: [[u16; 6]; 2],
    /// The mon whose move is running (side, slot).
    pub(crate) cur_user: Option<(u8, u8)>,
    /// The side of the last `|faint|` (checkWin's tie rule).
    pub(crate) last_faint: Option<u8>,
    /// Active slots (side*2+slot bits) whose Emergency Exit `-activate`
    /// printed during the current action.
    pub(crate) ee_announced: u8,
    /// The running move's `|move|` target (side, slot).
    pub(crate) move_target: Option<(u8, u8)>,
    /// The last residual handler ended by duration: fieldEvent `continue`s
    /// past its faintMessages (sim/battle.ts:515-521).
    pub(crate) res_defer_faint: bool,
    /// PS `side.pokemon` order (position -> team index); switches swap.
    pub(crate) pos: [[u8; 6]; 2],
    /// Illusion disguise per team index (255 = none).
    pub(crate) illusion: [[u8; 6]; 2],
}

/// One buffered entry of a reorder window.
#[derive(Debug, Clone)]
pub(crate) enum WinEntry {
    Line(String),
    Split(u8, String, String),
}

/// A reorder window: the engine resolves a spread move target by target,
/// PS step by step across all targets (sim/battle-actions.ts
/// trySpreadMoveHit / spreadMoveHit). Lines are tagged with the current
/// (hit, step, target rank) key and written in key order on close.
#[derive(Debug, Clone, Default)]
pub(crate) struct Window {
    pub(crate) entries: Vec<(u32, WinEntry)>,
    pub(crate) key: u32,
    /// Bit `rank`: the target reached the damage step (PS `hitTargets`).
    pub(crate) hit_ranks: u8,
    /// `p1a`-style slot of each target rank, for `[spread]`.
    pub(crate) rank_slots: [[u8; 3]; 4],
    pub(crate) n_ranks: u8,
    /// The hit number of the lines that follow (255 = after the loop).
    pub(crate) hit: u8,
}

/// Step tags for [`Window`] keys, in PS's order.
pub(crate) mod step {
    pub const PRE: u8 = 0;
    pub const INVULN: u8 = 1;
    pub const TRYHIT: u8 = 2;
    pub const TYPE_IMMUNE: u8 = 3;
    pub const TRY_IMMUNE: u8 = 4;
    pub const ACCURACY: u8 = 5;
    pub const BREAK_PROTECT: u8 = 6;
    pub const SUBSTITUTE: u8 = 7;
    pub const CALC: u8 = 8;
    pub const CALC_ITEM: u8 = 9;
    pub const DAMAGE: u8 = 10;
    pub const EFFECTS: u8 = 11;
    pub const SELF_DROP: u8 = 12;
    pub const SECONDARY: u8 = 13;
    pub const DAMAGING_HIT: u8 = 14;
    pub const AFTER_HIT: u8 = 15;
    pub const UPDATE: u8 = 16;
    // After the hit loop (`hit` = 255): faintMessages, `-hitcount`,
    // recoil, the move's AfterMoveSecondary (Eject Button, Red Card).
    pub const LOOP_FAINT: u8 = 20;
    pub const AFTER_FAINT: u8 = 21;
    pub const HITCOUNT: u8 = 22;
    pub const RECOIL: u8 = 23;
    pub const AFTER_SECONDARY: u8 = 24;
    pub const EMERGENCY_EXIT: u8 = 25;
    pub const SELF_BOOST: u8 = 26;
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
            faint_keys: Vec::new(),
            fainted_logged: 0,
            champions_hp: false,
            ots: false,
            switch_ctx: None,
            last_move: [[u16::MAX; 2]; 2],
            defrost_pending: false,
            boost_fx: BoostFx::Move,
            status_fx: StatusFx::Plain,
            active_move: u16::MAX,
            pending_move: None,
            window: None,
            hit_crit: false,
            hitcount: None,
            loop_ended: false,
            faint_prefix: Vec::new(),
            magic_room_source: String::new(),
            status_ctx: None,
            since_move: 0,
            res_speed: [[0; 2]; 2],
            res_hp_before: 0,
            trap_move: [[u16::MAX; 6]; 2],
            cur_user: None,
            last_faint: None,
            ee_announced: 0,
            move_target: None,
            res_defer_faint: false,
            pos: [[0, 1, 2, 3, 4, 5]; 2],
            illusion: [[255; 6]; 2],
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
        if self.ended {
            return;
        }
        self.flush_pending_move();
        self.since_move += 1;
        if let Some(w) = self.window.as_deref_mut() {
            w.entries.push((w.key, WinEntry::Line(line)));
            return;
        }
        self.lines.push(line);
    }

    /// Remove the last `n` plain lines if each starts with `prefix`
    /// (window-aware); `None` leaves the log unchanged.
    pub(crate) fn pop_tail_lines(&mut self, n: usize, prefix: &str) -> Option<Vec<String>> {
        if let Some(w) = self.window.as_deref_mut() {
            let len = w.entries.len();
            if len < n || !w.entries[len - n..].iter().all(|(_, e)| matches!(e, WinEntry::Line(l) if l.starts_with(prefix))) {
                return None;
            }
            return Some(w.entries.drain(len - n..).map(|(_, e)| match e { WinEntry::Line(l) => l, WinEntry::Split(_, s, _) => s }).collect());
        }
        let len = self.lines.len();
        if self.pending_move.is_some() || len < n || !self.lines[len - n..].iter().all(|l| l.starts_with(prefix)) {
            return None;
        }
        Some(self.lines.drain(len - n..).collect())
    }

    /// PS `addSplit(side, secret, shared)`: `shared = None` hides the line
    /// from everyone but `side` (and the omniscient channel).
    pub(crate) fn push_split(&mut self, side: u8, secret: String, shared: Option<String>) {
        if self.ended {
            return;
        }
        self.flush_pending_move();
        self.since_move += 1;
        if let Some(w) = self.window.as_deref_mut() {
            w.entries.push((w.key, WinEntry::Split(side, secret, shared.unwrap_or_default())));
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
        self.flush_pending_move();
        self.last_move_line = Some(self.lines.len());
        self.lines.push(line);
    }

    /// Write a pending `|move|` line (see `pending_move`).
    pub(crate) fn flush_pending_move(&mut self) {
        if let Some(l) = self.pending_move.take() {
            self.last_move_line = Some(self.lines.len());
            self.lines.push(l);
        }
    }

    /// Open a reorder window (see [`Window`]).
    pub(crate) fn window_open(&mut self) {
        self.flush_pending_move();
        if self.window.is_none() {
            self.window = Some(Box::default());
        }
    }

    /// Close the window: its lines, stably sorted by key, join the log.
    pub(crate) fn window_close(&mut self) {
        let Some(mut w) = self.window.take() else { return };
        w.entries.sort_by_key(|(k, _)| *k);
        for (_, e) in w.entries {
            match e {
                WinEntry::Line(l) => self.lines.push(l),
                WinEntry::Split(side, a, b) => {
                    self.lines.push(format!("|split|p{}", side + 1));
                    self.lines.push(a);
                    self.lines.push(b);
                }
            }
        }
    }

    /// PS `attrLastMove` (sim/battle.ts:3122-3140).
    pub(crate) fn attr_last_move(&mut self, attr: &str) {
        self.flush_pending_move();
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
        self.flush_pending_move();
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
