//! Pokémon Showdown's `BattleStream` (sim/battle-stream.ts) over the engine:
//! the text protocol `pokemon-showdown simulate-battle` speaks (docs/protocol.md,
//! "Use as a local Showdown replacement").
//!
//! Input is PS's: `>start {...}`, `>player p1 {...}`, `>p1 move 1 1, move 2`,
//! `>p1 team 1234`, `>p1 undo`, `>forcewin p1`, `>forcetie`, `>forcelose p2`,
//! `>tiebreak`, `>chat`, `>eval` (answered with an error: there is no JS to
//! run), `>requestlog` / `>requestexport` / `>requestteam`. Output is the
//! messages PS's BattleStream pushes: `update\n<log lines>`,
//! `sideupdate\npN\n<|request| or |error|>`, `end\n<json>` and
//! `requesteddata\n...`, each exactly as PS formats it. [`BattleStream::write`]
//! takes one chunk (PS processes a chunk's lines, then sends one update:
//! `BattleStream._write`, sim/battle-stream.ts:35-48).

use crate::battle::{Battle, BattleConfig};
use crate::protocol::{is_cosmetic, ProtocolMode, ProtocolOptions};
use crate::ps_formats::PsFormat;
use crate::ps_json::Json;
use crate::ps_session::PsSession;
use crate::ps_teams;
use crate::side::SideRef;
use crate::{Format, Rng};
use std::collections::VecDeque;

/// `--replay` / `--spectate` (sim/battle-stream.ts:56-69): only the log, as
/// one channel sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Replay {
    #[default]
    Off,
    /// The omniscient log (`--replay`).
    Omniscient,
    /// The spectator log (`--spectate`).
    Spectator,
}

/// BattleStream options.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub replay: Replay,
    /// `Ps` writes PS's bytes; `Compact` drops the cosmetic lines from
    /// `update`s (docs/protocol.md, Modes).
    pub mode: ProtocolMode,
    /// `keepAlive`: don't end the stream after `end`.
    pub keep_alive: bool,
    /// A fixed `|t:|` value (tests); `None` is wall-clock seconds, as PS.
    pub timestamp: Option<u64>,
}

struct Player {
    name: String,
    avatar: String,
    /// The team sets, as PS's `side.team` (the `end` record prints them).
    sets: Vec<Json>,
}

/// The BattleStream (see the module docs).
pub struct BattleStream {
    opts: StreamOptions,
    format: Option<&'static PsFormat>,
    seed: String,
    rated: Option<Json>,
    players: [Option<Player>; 2],
    session: Option<PsSession>,
    /// PS `battle.log`.
    log: Vec<String>,
    /// PS `sentLogPos`.
    sent_pos: usize,
    /// Session raw lines already copied into `log`.
    session_seen: usize,
    input_log: Vec<String>,
    sent_end: bool,
    /// PS `pushEnd` ran: nothing more is read.
    closed: bool,
    out: VecDeque<String>,
}

fn now(opts: &StreamOptions) -> u64 {
    opts.timestamp.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

/// `PRNG.generateSeed()` (sim/prng.ts:160-162, :210-224): 128 random bits.
fn generate_seed() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::from("sodium,");
    for k in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(k);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// `Utils.splitFirst(s, sep)`.
fn split_first<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str) {
    match s.find(sep) {
        Some(i) => (&s[..i], &s[i + sep.len()..]),
        None => (s, ""),
    }
}

fn side_of(id: &str) -> Option<SideRef> {
    match id {
        "p1" => Some(SideRef::P1),
        "p2" => Some(SideRef::P2),
        _ => None,
    }
}

/// `extractChannelMessages(message, [channel])` (sim/battle.ts:33-59):
/// channel -1 is omniscient, 0 spectator, 1 / 2 a player.
pub fn extract_channel(message: &str, channel: i8) -> String {
    let lines: Vec<&str> = message.split('\n').collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        // /^\|split\|p([1234])\n(.*)\n(.*)/m needs both following lines.
        if let Some(p) = l.strip_prefix("|split|p").filter(|p| p.len() == 1 && matches!(p.as_bytes()[0], b'1'..=b'4')) {
            if i + 2 < lines.len() {
                let player: i8 = p.parse().unwrap_or(0);
                let line = if channel == -1 || player == channel { lines[i + 1] } else { lines[i + 2] };
                if !line.is_empty() {
                    out.push(line);
                }
                i += 3;
                continue;
            }
        }
        if !l.is_empty() {
            out.push(l);
        }
        i += 1;
    }
    out.join("\n")
}

/// Drop the cosmetic lines (and `|split|` triples whose secret half is
/// cosmetic) from an update's lines.
fn compact(lines: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if l.starts_with("|split|p") && i + 2 < lines.len() {
            if !is_cosmetic(&lines[i + 1]) {
                out.extend_from_slice(&lines[i..i + 3]);
            }
            i += 3;
            continue;
        }
        if !is_cosmetic(l) {
            out.push(l.clone());
        }
        i += 1;
    }
    out
}

impl BattleStream {
    pub fn new(opts: StreamOptions) -> Self {
        Self {
            opts,
            format: None,
            seed: String::new(),
            rated: None,
            players: [None, None],
            session: None,
            log: Vec::new(),
            sent_pos: 0,
            session_seen: 0,
            input_log: Vec::new(),
            sent_end: false,
            closed: false,
            out: VecDeque::new(),
        }
    }

    /// The next message PS's BattleStream would yield (`"update\n..."`, ...).
    pub fn read(&mut self) -> Option<String> {
        self.out.pop_front()
    }

    /// Every pending message.
    pub fn drain(&mut self) -> Vec<String> {
        self.out.drain(..).collect()
    }

    /// The stream has ended (PS `pushEnd`: after `end`, unless keepAlive).
    pub fn is_closed(&self) -> bool {
        self.closed && self.out.is_empty()
    }

    /// The session, once both players have joined.
    pub fn session(&self) -> Option<&PsSession> {
        self.session.as_ref()
    }

    /// PS `battle.inputLog`.
    pub fn input_log(&self) -> &[String] {
        &self.input_log
    }

    /// PS `BattleStream._write` (sim/battle-stream.ts:35-54): every `>`
    /// line of the chunk, then one `sendUpdates`. `Err` is what PS throws
    /// (with `noCatch`, `simulate-battle` dies on it).
    pub fn write(&mut self, chunk: &str) -> Result<(), String> {
        for line in chunk.split('\n') {
            if let Some(rest) = line.strip_prefix('>') {
                let (ty, message) = split_first(rest, " ");
                self.write_line(ty, message)?;
            }
        }
        self.send_updates();
        Ok(())
    }

    fn push(&mut self, ty: &str, data: &str) {
        if self.closed {
            return;
        }
        match self.opts.replay {
            Replay::Off => self.out.push_back(format!("{ty}\n{data}")),
            r => {
                if ty == "update" {
                    let ch = if r == Replay::Spectator { 0 } else { -1 };
                    self.out.push_back(extract_channel(data, ch));
                }
            }
        }
        if ty == "end" && !self.opts.keep_alive {
            self.closed = true;
        }
    }

    fn side_messages(&mut self, side: SideRef, m: crate::ps_session::SideMessages) {
        let id = if side == SideRef::P1 { "p1" } else { "p2" };
        for e in m.errors {
            self.push("sideupdate", &format!("{id}\n{e}"));
        }
        if let Some(r) = m.updated_request {
            self.push("sideupdate", &format!("{id}\n|request|{r}"));
        }
    }

    /// Copy the session's newly exposed log lines (and committed choices).
    fn pull_session(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let raw = s.raw_lines();
        if raw.len() > self.session_seen {
            let new: Vec<String> = raw[self.session_seen..].to_vec();
            self.session_seen = raw.len();
            for l in new {
                let rated = l.starts_with("|tier|") && self.rated.is_some();
                self.log.push(l);
                // `|rated|` follows `|tier|` (sim/battle.ts:1937-1941).
                if rated {
                    let r = match self.rated.as_ref() {
                        Some(Json::Str(s)) if s != "Rated battle" => s.clone(),
                        _ => String::new(),
                    };
                    self.log.push(format!("|rated|{r}"));
                }
            }
        }
        let s = self.session.as_mut().expect("checked");
        self.input_log.extend(s.take_input_log());
    }

    fn session_mut(&mut self) -> Result<&mut PsSession, String> {
        self.session.as_mut().ok_or_else(|| "Cannot read properties of null (the battle has no players yet)".to_string())
    }

    fn write_line(&mut self, ty: &str, message: &str) -> Result<(), String> {
        match ty {
            "start" => self.start(message)?,
            "player" => {
                let (slot, text) = split_first(message, " ");
                let opts = Json::parse(text)?;
                self.set_player(slot, opts)?;
            }
            "p1" | "p2" | "p3" | "p4" => {
                let side = side_of(ty).ok_or_else(|| format!("Cannot read properties of undefined (side {ty})"))?;
                let s = self.session_mut()?;
                let m = if message == "undo" { s.undo(side) } else { s.choose_detailed(side, message) };
                self.side_messages(side, m);
                self.pull_session();
            }
            "forcewin" | "forcetie" => {
                let side = if ty == "forcewin" { side_of(message) } else { None };
                self.session_mut()?.force_win(side);
                self.pull_session();
                self.input_log.push(if message.is_empty() { ">forcetie".to_string() } else { format!(">forcewin {message}") });
            }
            "forcelose" => {
                if let Some(side) = side_of(message) {
                    self.session_mut()?.force_win(Some(side.opposing()));
                    self.pull_session();
                }
                self.input_log.push(format!(">forcelose {message}"));
            }
            "reseed" => {
                let seed = if message.is_empty() { generate_seed() } else { message.to_string() };
                let rng = make_rng(&seed)?;
                self.session_mut()?.reseed(rng);
                self.input_log.push(format!(">reseed {seed}"));
            }
            "tiebreak" => self.tiebreak()?,
            "chat-inputlogonly" => self.input_log.push(format!(">chat {message}")),
            "chat" => {
                self.input_log.push(format!(">chat {message}"));
                self.log.push(format!("|chat|{message}"));
            }
            "eval" => {
                self.input_log.push(format!(">{ty} {message}"));
                let m = message.replace('\u{c}', "\n");
                self.log.push(format!("||>>> {}", m.replace('\n', "\n||")));
                self.log.push("||<<< error: vgc-engine has no JavaScript to evaluate".to_string());
            }
            "editbattle" => self.log.push("||<<< error: vgc-engine does not support /editbattle".to_string()),
            "requestlog" => {
                let d = self.input_log.join("\n");
                self.out.push_back(format!("requesteddata\n{d}"));
            }
            "requestexport" => {
                let d = format!("{}\n{}", self.seed, self.input_log.join("\n"));
                self.out.push_back(format!("requesteddata\n{d}"));
            }
            "requestteam" => {
                let slot = js_slot(message.trim()).ok_or_else(|| format!("Team requested for slot {message}, but that slot does not exist."))?;
                let team = self.players.get(slot).and_then(|p| p.as_ref()).map(|p| ps_teams::pack(&p.sets)).unwrap_or_default();
                self.out.push_back(format!("requesteddata\n{team}"));
            }
            "show-openteamsheets" => self.show_open_team_sheets(),
            "version" | "version-origin" => {}
            _ => return Err(format!("Unrecognized command \">{ty} {message}\"")),
        }
        Ok(())
    }

    /// `>start` (sim/battle-stream.ts:73-82, sim/battle.ts:191-305).
    fn start(&mut self, message: &str) -> Result<(), String> {
        let opts = Json::parse(message)?;
        let fid = opts.str_of("formatid");
        let format = crate::ps_formats::lookup(fid).ok_or_else(|| format!("Unidentified format: {fid} (vgc-engine knows gen 9 and Champions formats)"))?;
        self.seed = match opts.get("seed") {
            Some(Json::Str(s)) if !s.is_empty() => s.clone(),
            Some(Json::Arr(a)) if !a.is_empty() => a.iter().map(|v| match v {
                Json::Num(n) => crate::ps_json::js_number(*n),
                Json::Str(s) => s.clone(),
                _ => String::new(),
            }).collect::<Vec<_>>().join(","),
            _ => generate_seed(),
        };
        make_rng(&self.seed)?;
        self.format = Some(format);
        self.rated = opts.get("rated").filter(|r| r.truthy()).cloned();
        let mut input = Json::obj();
        if let Some(f) = opts.get("formatid") {
            input.set("formatid", f.clone());
        }
        input.set("seed", Json::Str(self.seed.clone()));
        if let Some(r) = &self.rated {
            input.set("rated", if matches!(r, Json::Str(s) if s == "Rated battle") { Json::Bool(true) } else { r.clone() });
        }
        self.input_log.push(format!(">start {}", input.stringify()));
        self.log.push(format!("|t:|{}", now(&self.opts)));
        self.log.push(format!("|gametype|{}", if format.doubles { "doubles" } else { "singles" }));
        for (k, id) in ["p1", "p2"].into_iter().enumerate() {
            if let Some(p) = opts.get(id).filter(|p| p.truthy()) {
                let _ = k;
                self.set_player(id, p.clone())?;
            }
        }
        Ok(())
    }

    /// `>player` (sim/battle.ts:3226-3260).
    fn set_player(&mut self, slot: &str, mut opts: Json) -> Result<(), String> {
        let Some(format) = self.format else { return Err("Cannot read properties of null (reading 'setPlayer')".into()) };
        let k = match slot {
            "p1" => 0,
            "p2" => 1,
            _ => return Err(format!("vgc-engine plays two-player battles; got {slot}")),
        };
        let name_of = |o: &Json, k: usize| match o.get("name").filter(|n| n.truthy()) {
            Some(Json::Str(s)) => s.clone(),
            Some(v) => crate::ps_json::Json::stringify(v).trim_matches('"').to_string(),
            None => format!("Player {}", k + 1),
        };
        let avatar_of = |o: &Json| match o.get("avatar").filter(|a| a.truthy()) {
            Some(Json::Str(s)) => s.clone(),
            Some(Json::Num(n)) => crate::ps_json::js_number(*n),
            Some(v) => v.stringify(),
            None => String::new(),
        };
        if let Some(p) = self.players[k].as_mut() {
            // Edit a player.
            if opts.get("team").is_some_and(Json::truthy) {
                return Err(format!("Player {slot} already has a team!"));
            }
            let mut changed = false;
            let name = name_of(&opts, k);
            if opts.get("name").is_some_and(Json::truthy) && p.name != name {
                p.name = name;
                changed = true;
            }
            let avatar = avatar_of(&opts);
            if !avatar.is_empty() && p.avatar != avatar {
                p.avatar = avatar;
                changed = true;
            }
            if !changed {
                return Ok(());
            }
        } else {
            let mut sets = match opts.get("team") {
                Some(Json::Str(t)) if !t.is_empty() => ps_teams::unpack(t).ok_or_else(|| "vgc-engine can't generate random teams: the team did not unpack".to_string())?,
                Some(Json::Arr(a)) if !a.is_empty() => a.clone(),
                _ => return Err("vgc-engine can't generate random teams; give the player a team".into()),
            };
            for s in &mut sets {
                ps_teams::normalize_set(s)?;
            }
            if matches!(opts.get("team"), Some(Json::Arr(_))) {
                opts.set("team", Json::Str(ps_teams::pack(&sets)));
            }
            self.players[k] = Some(Player { name: name_of(&opts, k), avatar: avatar_of(&opts), sets });
        }
        let _ = format;
        self.input_log.push(format!(">player {slot} {}", opts.stringify()));
        let rating = match opts.get("rating").filter(|r| r.truthy()) {
            Some(Json::Str(s)) => s.clone(),
            Some(Json::Num(n)) => crate::ps_json::js_number(*n),
            Some(v) => v.stringify(),
            None => String::new(),
        };
        let p = self.players[k].as_ref().expect("set");
        self.log.push(format!("|player|{slot}|{}|{}|{rating}", p.name, p.avatar));
        if self.players.iter().all(Option::is_some) && self.session.is_none() {
            self.begin()?;
        }
        Ok(())
    }

    /// Both players are in: PS `start()` (sim/battle.ts:1905-1969).
    fn begin(&mut self) -> Result<(), String> {
        let format = self.format.expect("started");
        let mut teams: [Vec<crate::Pokemon>; 2] = [Vec::new(), Vec::new()];
        let mut nicks: [Vec<Option<String>>; 2] = [Vec::new(), Vec::new()];
        for k in 0..2 {
            let p = self.players[k].as_ref().expect("both players");
            for s in &p.sets {
                let m = ps_teams::to_member(s);
                let mon = crate::build_member_in(&m, format.champions()).map_err(|e| e.to_string())?;
                teams[k].push(mon);
                nicks[k].push(Some(ps_teams::display_name(s)));
            }
        }
        let names = [self.players[0].as_ref().unwrap().name.clone(), self.players[1].as_ref().unwrap().name.clone()];
        let mode = ProtocolMode::Ps;
        let mut opts = ProtocolOptions::new(mode);
        opts.player_names = names;
        opts.nicknames = nicks;
        opts.timestamp = self.opts.timestamp;
        let cfg = BattleConfig { format: if format.doubles { Format::Doubles } else { Format::Singles }, seed: seed_hash(&self.seed) };
        let champions = format.champions();
        let id = format.id;
        let seed = self.seed.clone();
        let [p1, p2] = teams;
        let builder = move |picks: Option<&[Vec<u8>; 2]>| {
            let rng = make_rng(&seed).expect("seed checked at >start");
            let mut b = Battle::with_protocol(cfg.clone(), rng, p1.clone(), p2.clone(), &opts, id, picks);
            b.decision_phases = true;
            b.champions = champions;
            b
        };
        let mut session = PsSession::new(Box::new(builder));
        session.set_support_cancel(format.support_cancel);
        // The battle's own preamble repeats `|t:|`, `|gametype|` and the
        // `|player|` lines this stream already sent at `>start` / `>player`.
        let skip = session.raw_lines().iter().take_while(|l| l.starts_with("|t:|") || l.starts_with("|gametype|") || l.starts_with("|player|")).count();
        self.session_seen = skip;
        self.session = Some(session);
        self.pull_session();
        Ok(())
    }

    /// `>tiebreak` (sim/battle.ts:1467-1508).
    fn tiebreak(&mut self) -> Result<(), String> {
        let names: Vec<String> = (0..2).map(|k| self.players[k].as_ref().map(|p| p.name.clone()).unwrap_or_default()).collect();
        let s = self.session.as_ref().ok_or("Cannot read properties of null (the battle has no players yet)")?;
        if s.is_ended() {
            return Ok(());
        }
        let b = s.battle();
        let stats: Vec<(usize, f64, f64)> = [SideRef::P1, SideRef::P2]
            .iter()
            .map(|&side| {
                let team = &b.side(side).team;
                let left = team.iter().filter(|m| m.current_hp > 0).count();
                // `pokemon.hp / pokemon.maxhp` summed in side order, then `* 100 / 6`.
                let frac: f64 = team.iter().map(|m| m.current_hp as f64 / m.stats.hp.max(1) as f64).fold(0.0, |a, x| a + x);
                let total: f64 = team.iter().map(|m| m.current_hp as f64).fold(0.0, |a, x| a + x);
                (left, frac * 100.0 / 6.0, total)
            })
            .collect();
        let mut lines = vec!["|message|Time's up! Going to tiebreaker...".to_string()];
        lines.push(format!("|-message|{}", (0..2).map(|k| format!("{}: {} Pokemon left", names[k], stats[k].0)).collect::<Vec<_>>().join("; ")));
        let js_round = |x: f64| (x + 0.5).floor();
        let mut tied: Vec<usize> = (0..2).collect();
        let max_left = stats.iter().map(|s| s.0).max().unwrap_or(0);
        tied.retain(|&k| stats[k].0 == max_left);
        let mut winner: Option<Option<usize>> = (tied.len() <= 1).then(|| tied.first().copied());
        if winner.is_none() {
            lines.push(format!("|-message|{}", tied.iter().map(|&k| format!("{}: {}% total HP left", names[k], js_round(stats[k].1))).collect::<Vec<_>>().join("; ")));
            let m = tied.iter().map(|&k| stats[k].1).fold(f64::MIN, f64::max);
            tied.retain(|&k| stats[k].1 == m);
            if tied.len() <= 1 {
                winner = Some(tied.first().copied());
            }
        }
        if winner.is_none() {
            lines.push(format!("|-message|{}", tied.iter().map(|&k| format!("{}: {} total HP left", names[k], js_round(stats[k].2))).collect::<Vec<_>>().join("; ")));
            let m = tied.iter().map(|&k| stats[k].2).fold(f64::MIN, f64::max);
            tied.retain(|&k| stats[k].2 == m);
            winner = Some(if tied.len() <= 1 { tied.first().copied() } else { None });
        }
        let w = winner.flatten().map(|k| if k == 0 { SideRef::P1 } else { SideRef::P2 });
        self.input_log.push(">tiebreak".to_string());
        self.session.as_mut().expect("checked").tiebreak(&lines, w);
        self.pull_session();
        Ok(())
    }

    /// `>show-openteamsheets` (sim/battle.ts:3184-3224), at turn 0.
    fn show_open_team_sheets(&mut self) {
        let (Some(s), Some(format)) = (self.session.as_ref(), self.format) else { return };
        if s.battle().turn() != 0 || s.raw_lines().iter().any(|l| l.starts_with("|turn|")) {
            return;
        }
        let mut lines = Vec::new();
        for (k, side) in [SideRef::P1, SideRef::P2].into_iter().enumerate() {
            let Some(p) = self.players[k].as_ref() else { continue };
            let team: Vec<Json> = p
                .sets
                .iter()
                .enumerate()
                .map(|(i, set)| {
                    let mut n = Json::obj();
                    n.set("name", Json::Str(String::new()));
                    for key in ["species", "item", "ability", "moves"] {
                        n.set(key, set.get(key).cloned().unwrap_or(Json::Null));
                    }
                    n.set("nature", if format.champions() { set.get("nature").cloned().unwrap_or(Json::Null) } else { Json::Str(String::new()) });
                    let g = match s.battle().side(side).team.get(i).map(|m| m.gender) {
                        Some(crate::Gender::Male) => "M",
                        Some(crate::Gender::Female) => "F",
                        _ => "",
                    };
                    n.set("gender", Json::Str(g.into()));
                    n.set("evs", Json::Null);
                    n.set("ivs", Json::Null);
                    n.set("level", set.get("level").cloned().unwrap_or(Json::Null));
                    if let Some(t) = set.get("teraType") {
                        n.set("teraType", t.clone());
                    }
                    n
                })
                .collect();
            lines.push(format!("|showteam|p{}|{}", k + 1, ps_teams::pack(&team)));
        }
        self.log.extend(lines);
    }

    /// PS `sendUpdates` (sim/battle.ts:3267-3309).
    fn send_updates(&mut self) {
        if self.sent_pos >= self.log.len() {
            return;
        }
        let mut lines: Vec<String> = self.log[self.sent_pos..].to_vec();
        if self.opts.mode == ProtocolMode::Compact {
            lines = compact(&lines);
        }
        self.push("update", &lines.join("\n"));
        self.sent_pos = self.log.len();
        let new_requests = self.session.as_mut().is_some_and(|s| s.take_new_requests());
        if new_requests {
            for side in [SideRef::P1, SideRef::P2] {
                let r = self.session.as_ref().and_then(|s| s.request_json(side)).unwrap_or_else(|| "null".into());
                let id = if side == SideRef::P1 { "p1" } else { "p2" };
                self.push("sideupdate", &format!("{id}\n|request|{r}"));
            }
        }
        let ended = self.session.as_ref().is_some_and(PsSession::is_ended);
        if ended && !self.sent_end {
            let rec = self.end_record();
            self.sent_end = true;
            self.push("end", &rec);
        }
    }

    /// The `end` record (sim/battle.ts:3276-3307).
    fn end_record(&self) -> String {
        let s = self.session.as_ref().expect("ended");
        let b = s.battle();
        let name = |k: usize| self.players[k].as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let winner = match b.winner() {
            Some(Some(SideRef::P1)) => name(0),
            Some(Some(SideRef::P2)) => name(1),
            _ => match s.raw_lines().iter().rev().find(|l| l.starts_with("|win|") || *l == "|tie") {
                Some(l) if l.starts_with("|win|") => l[5..].to_string(),
                _ => String::new(),
            },
        };
        let turns = s.raw_lines().iter().rev().find_map(|l| l.strip_prefix("|turn|").and_then(|t| t.parse::<f64>().ok())).unwrap_or(0.0);
        let left = |side: SideRef| b.side(side).team.iter().filter(|m| m.current_hp > 0).count() as f64;
        let mut rec = Json::obj();
        rec.set("winner", Json::Str(winner));
        rec.set("seed", Json::Str(self.seed.clone()));
        rec.set("turns", Json::Num(turns));
        rec.set("p1", Json::Str(name(0)));
        rec.set("p2", Json::Str(name(1)));
        for k in 0..2 {
            let sets = self.players[k].as_ref().map(|p| p.sets.clone()).unwrap_or_default();
            rec.set(&format!("p{}team", k + 1), Json::Arr(sets));
        }
        rec.set("score", Json::Arr(vec![Json::Num(left(SideRef::P1)), Json::Num(left(SideRef::P2))]));
        rec.set("inputLog", Json::Arr(self.input_log.iter().map(|l| Json::Str(l.clone())).collect()));
        rec.stringify()
    }
}

/// `requestteam`'s slot: `parseInt(message.slice(1)) - 1`.
fn js_slot(m: &str) -> Option<usize> {
    let d: String = m.chars().skip(1).take_while(|c| c.is_ascii_digit()).collect();
    d.parse::<usize>().ok().and_then(|n| n.checked_sub(1))
}

fn seed_hash(seed: &str) -> u64 {
    // FNV-1a: the SplitMix seed of a build without `ps-rng`.
    seed.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// The battle RNG for a PS seed: PS's own PRNG with `ps-rng`, else a
/// SplitMix stream seeded from it (deterministic, but not PS's draws).
pub fn make_rng(seed: &str) -> Result<Rng, String> {
    #[cfg(feature = "ps-rng")]
    {
        Rng::ps(seed).map_err(|e| format!("Unrecognized RNG seed {seed}: {e}"))
    }
    #[cfg(not(feature = "ps-rng"))]
    {
        Ok(Rng::new(seed_hash(seed)))
    }
}

/// `pokemon-showdown simulate-battle` (pokemon-showdown:199-215, a
/// `BattleTextStream` between stdin and stdout): each read from `input`
/// is one chunk, written to the battle up to its last newline (the rest
/// waits for the next read, sim/battle-stream.ts:523-530); every message
/// goes to `output` framed and flushed. Returns at end of input; input
/// after the stream ended (`end` sent) is read and ignored, as PS's
/// process keeps stdin open. `Err` is what PS throws.
pub fn run_simulate_battle(opts: StreamOptions, mut input: impl std::io::Read, mut output: impl std::io::Write) -> Result<(), String> {
    let mut s = BattleStream::new(opts);
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        if s.is_closed() {
            continue;
        }
        pending.extend_from_slice(&buf[..n]);
        let Some(i) = pending.iter().rposition(|&b| b == b'\n') else { continue };
        let chunk: Vec<u8> = pending.drain(..=i).collect();
        let text = String::from_utf8_lossy(&chunk[..chunk.len() - 1]).into_owned();
        let res = s.write(&text);
        for m in s.drain() {
            output.write_all(text_frame(&m).as_bytes()).map_err(|e| e.to_string())?;
        }
        output.flush().map_err(|e| e.to_string())?;
        res?;
    }
}

/// Frame a BattleStream message as `BattleTextStream` writes it to stdout
/// (sim/battle-stream.ts:516-521): a trailing newline, then a blank line.
pub fn text_frame(message: &str) -> String {
    let mut m = message.to_string();
    if !m.ends_with('\n') {
        m.push('\n');
    }
    m.push('\n');
    m
}

/// Route one BattleStream message the way `getPlayerStreams` does
/// (sim/battle-stream.ts:395-462): `(view, chunk)` pairs for views 0 =
/// omniscient, 1 = spectator, 2 = p1, 3 = p2.
pub fn player_stream_chunks(message: &str) -> Vec<(usize, String)> {
    let (ty, data) = split_first(message, "\n");
    match ty {
        "update" => vec![(0, extract_channel(data, -1)), (1, extract_channel(data, 0)), (2, extract_channel(data, 1)), (3, extract_channel(data, 2))],
        "sideupdate" => {
            let (side, d) = split_first(data, "\n");
            match side {
                "p1" => vec![(2, d.to_string())],
                "p2" => vec![(3, d.to_string())],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = "Incineroar||SitrusBerry|Intimidate|FakeOut,FlareBlitz,PartingShot,Protect||252,,,,,||||50|]Amoonguss||RockyHelmet|Regenerator|Spore,RagePowder,PollenPuff,Protect||||||50|]Rillaboom||AssaultVest|GrassySurge|FakeOut,GrassyGlide,WoodHammer,Uturn||||||50|]Urshifu-Rapid-Strike||MysticWater|UnseenFist|SurgingStrikes,CloseCombat,AquaJet,Protect||||||50|";

    fn stream() -> BattleStream {
        let mut s = BattleStream::new(StreamOptions { timestamp: Some(1), ..Default::default() });
        s.write(r#">start {"formatid":"gen9vgc2025regi","seed":"sodium,00000000000000000000000000000001"}"#).unwrap();
        s.write(&format!(r#">player p1 {{"name":"A","team":"{TEAM}"}}"#)).unwrap();
        s.write(&format!(r#">player p2 {{"name":"B","team":"{TEAM}"}}"#)).unwrap();
        s
    }

    #[test]
    fn framing_preamble_and_requests() {
        let mut s = stream();
        let m = s.drain();
        assert_eq!(m[0], "update\n|t:|1\n|gametype|doubles");
        assert_eq!(m[1], "update\n|player|p1|A||");
        assert!(m[2].starts_with("update\n|player|p2|B||\n|gen|9\n|tier|[Gen 9] VGC 2025 Reg I\n|rule|Species Clause"), "{}", m[2]);
        assert!(m[3].starts_with("sideupdate\np1\n|request|{\"teamPreview\":true,\"maxChosenTeamSize\":4"));
        assert!(m[4].starts_with("sideupdate\np2\n|request|"));
        // A choice for one side sends nothing; a bad one an error.
        s.write(">p1 team 1234").unwrap();
        assert!(s.drain().is_empty());
        s.write(">p1 undo").unwrap();
        assert!(s.drain().is_empty(), "Cancel Mod: undo is silent");
        s.write(">p2 team 9").unwrap();
        assert_eq!(s.drain(), vec!["sideupdate\np2\n|error|[Invalid choice] Can't choose for Team Preview: You do not have a Pokémon in slot 9"]);
        s.write(">p1 team 1234\n>p2 team 2134").unwrap();
        let m = s.drain();
        assert!(m[0].starts_with("update\n|\n|t:|1\n|teamsize|p1|4"), "{}", m[0]);
        assert!(m[1].starts_with("sideupdate\np1\n|request|{\"active\""));
        s.write(">forcetie").unwrap();
        let m = s.drain();
        assert_eq!(m[0], "update\n|\n|tie");
        assert!(m[1].starts_with("end\n{\"winner\":\"\",\"seed\":\"sodium,00000000000000000000000000000001\",\"turns\":1,\"p1\":\"A\",\"p2\":\"B\",\"p1team\":[{\"name\":\"Incineroar\""));
        assert!(m[1].ends_with(r#""score":[4,4],"inputLog":[">start {\"formatid\":\"gen9vgc2025regi\",\"seed\":\"sodium,00000000000000000000000000000001\"}",">player p1 {\"name\":\"A\",\"team\":\"Incineroar||SitrusBerry|Intimidate|FakeOut,FlareBlitz,PartingShot,Protect||252,,,,,||||50|]Amoonguss||RockyHelmet|Regenerator|Spore,RagePowder,PollenPuff,Protect||||||50|]Rillaboom||AssaultVest|GrassySurge|FakeOut,GrassyGlide,WoodHammer,Uturn||||||50|]Urshifu-Rapid-Strike||MysticWater|UnseenFist|SurgingStrikes,CloseCombat,AquaJet,Protect||||||50|\"}",">player p2 {\"name\":\"B\",\"team\":\"Incineroar||SitrusBerry|Intimidate|FakeOut,FlareBlitz,PartingShot,Protect||252,,,,,||||50|]Amoonguss||RockyHelmet|Regenerator|Spore,RagePowder,PollenPuff,Protect||||||50|]Rillaboom||AssaultVest|GrassySurge|FakeOut,GrassyGlide,WoodHammer,Uturn||||||50|]Urshifu-Rapid-Strike||MysticWater|UnseenFist|SurgingStrikes,CloseCombat,AquaJet,Protect||||||50|\"}",">p1 team 1, 2, 3, 4",">p2 team 2, 1, 3, 4",">forcetie"]}"#), "{}", m[1]);
        assert!(s.is_closed());
    }

    #[test]
    fn player_streams_split_like_ps() {
        let v = player_stream_chunks("update\n|split|p1\n|switch|a|b|10/20\n|switch|a|b|50/100\n|turn|1");
        assert_eq!(v[0], (0, "|switch|a|b|10/20\n|turn|1".to_string()));
        assert_eq!(v[1], (1, "|switch|a|b|50/100\n|turn|1".to_string()));
        assert_eq!(v[2], (2, "|switch|a|b|10/20\n|turn|1".to_string()));
        assert_eq!(v[3], (3, "|switch|a|b|50/100\n|turn|1".to_string()));
        assert_eq!(player_stream_chunks("sideupdate\np2\n|request|{}"), vec![(3, "|request|{}".to_string())]);
        assert_eq!(text_frame("update\nx"), "update\nx\n\n");
    }
}
