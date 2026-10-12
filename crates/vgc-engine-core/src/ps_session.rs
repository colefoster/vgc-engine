//! A Pokémon Showdown-style choice/request driver over a logged battle
//! (docs/protocol.md, phase 2).
//!
//! [`PsSession`] plays a battle the way PS's `BattleStream` does for its
//! players: each side reads its own stream (updates with `|split|` resolved
//! for it, then its `|request|`), answers in PS choice syntax (`move 1 2,
//! switch 3`, `team 1234`, `pass`, `default`), and gets PS's exact
//! `|error|` text back for a bad choice (sim/side.ts `choose`).
//!
//! The engine resolves a turn in one `step` with mid-turn picks (U-turn,
//! Eject Button, Revival Blessing) supplied up front. PS instead stops the
//! turn and asks. The session replays: it runs the turn from the stored
//! start-of-turn battle with the picks made so far; the first unanswered
//! mid-turn stop the replay records is where PS would be waiting, so the
//! streams stop there. A pick reruns the turn with it appended. Replays are
//! deterministic (same RNG state, same choices), so the exposed prefix never
//! changes.

use crate::battle::{Battle, StepResult};
use crate::choice::{Choice, Target, STRUGGLE_MOVE_SLOT};
use crate::protocol::{ProtocolView, SyncKind, SyncPoint};
use crate::ps_request::{ReqState, SideReq};
use crate::side::SideRef;

/// Builds the battle from team-preview picks (`None` = no picks yet; the
/// battle is only read up to its team-preview stop then).
pub type BattleBuilder = Box<dyn Fn(Option<&[Vec<u8>; 2]>) -> Battle + Send + Sync>;

const VIEWS: [ProtocolView; 4] = [ProtocolView::P1, ProtocolView::P2, ProtocolView::Spectator, ProtocolView::Omniscient];

/// `|error|` text for a choice or undo after a `cantUndo` choice
/// (sim/side.ts:1192-1194, sim/battle.ts:3036-3039).
const CANT_UNDO: &str = "|error|[Invalid choice] Can't undo: A trapping/disabling effect would cause undo to leak information";

/// What one `choose` / `undo` call sent the side, in PS's order: `|error|`
/// lines, then (for an `[Unavailable choice]` or an undo that revealed
/// something) the updated `|request|` JSON (sim/side.ts:529-537).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideMessages {
    pub errors: Vec<String>,
    pub updated_request: Option<String>,
}

/// The session (see the module docs).
pub struct PsSession {
    builder: BattleBuilder,
    /// The battle at the start of the current decision (the turn start for
    /// a mid-turn stop).
    base: Battle,
    /// The current stop.
    stop: SyncPoint,
    /// The current requests (an `[Unavailable choice]` updates them).
    reqs: [SideReq; 2],
    /// This turn's main choices and the mid-turn picks so far.
    main: [Vec<Choice>; 2],
    mid: [Vec<Choice>; 2],
    mid_answered: usize,
    /// A side's accepted answer to the current request.
    answered: [Option<Answer>; 2],
    /// The accepted answer as PS's `side.getChoice()` prints it
    /// (sim/side.ts:323-351), for the input log.
    choice_text: [Option<String>; 2],
    /// PS `side.choice.cantUndo`.
    cant_undo: [bool; 2],
    /// Cancel Mod (data/rulesets.ts:1371-1378): without it every accepted
    /// choice is final (sim/battle.ts:3060-3068).
    support_cancel: bool,
    /// Chunks per view (p1, p2, spectator, omniscient).
    streams: [Vec<String>; 4],
    /// Log-derived chunks already in each stream.
    log_chunks: [usize; 4],
    /// The raw `battle.log` lines exposed so far (`|split|` pairs included).
    raw: Vec<String>,
    /// How many of the log's raw lines `raw` holds.
    raw_pos: usize,
    /// A stop's requests were made and not yet taken (PS `sentRequests`).
    new_requests: bool,
    /// `>pN <choice>` lines PS's commitChoices logged (sim/battle.ts:3009-3012).
    input_log: Vec<String>,
    ended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Choices(Vec<Choice>),
    Team(Vec<u8>),
}

impl PsSession {
    /// Start a session. `builder(None)` must give the battle at its first
    /// stop (team preview, or turn 1 without preview); the session always
    /// runs with `decision_phases` on.
    pub fn new(builder: BattleBuilder) -> Self {
        let mut base = builder(None);
        base.decision_phases = true;
        let stop = base
            .protocol_log()
            .and_then(|l| l.sync_points().first().cloned())
            .expect("PsSession needs a battle built with a protocol log");
        let reqs = (*stop.info).clone();
        let mut s = Self {
            builder,
            base,
            stop,
            reqs,
            main: [Vec::new(), Vec::new()],
            mid: [Vec::new(), Vec::new()],
            mid_answered: 0,
            answered: [None, None],
            choice_text: [None, None],
            cant_undo: [false; 2],
            support_cancel: true,
            streams: Default::default(),
            log_chunks: [0; 4],
            raw: Vec::new(),
            raw_pos: 0,
            new_requests: true,
            input_log: Vec::new(),
            ended: false,
        };
        let line = s.stop.line;
        let b = s.take_base();
        s.expose(&b, Some(line));
        s.base = b;
        s
    }

    /// Cancel Mod off: an accepted choice can't be changed or undone.
    pub fn set_support_cancel(&mut self, on: bool) {
        self.support_cancel = on;
    }

    /// The committed battle state (start of the current turn while a
    /// mid-turn request is open).
    pub fn battle(&self) -> &Battle {
        &self.base
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Append the chunks of `b`'s log up to raw line `upto` (all with `None`).
    fn expose(&mut self, b: &Battle, upto: Option<usize>) {
        let Some(log) = b.protocol_log() else { return };
        for (k, view) in VIEWS.into_iter().enumerate() {
            let chunks = log.stream(view, upto);
            for c in chunks.iter().skip(self.log_chunks[k]) {
                self.streams[k].push(c.text().to_string());
            }
            self.log_chunks[k] = self.log_chunks[k].max(chunks.len());
        }
        let lines = log.raw_lines();
        let end = upto.unwrap_or(lines.len()).min(lines.len());
        if end > self.raw_pos {
            self.raw.extend(lines[self.raw_pos..end].iter().cloned());
            self.raw_pos = end;
        }
    }

    /// The raw `battle.log` exposed so far: what PS has sent in `update`s
    /// (with `|split|` pairs), up to the current stop.
    pub fn raw_lines(&self) -> &[String] {
        &self.raw
    }

    /// True once per stop: the stop's requests are still to be sent
    /// (PS `sentRequests`, sim/battle.ts:3267-3274).
    pub fn take_new_requests(&mut self) -> bool {
        std::mem::take(&mut self.new_requests)
    }

    /// The `>pN <choice>` input-log lines committed since the last call.
    pub fn take_input_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.input_log)
    }

    /// `side`'s current request JSON, or `None` when it has nothing to
    /// choose: a `wait` request, an answer already accepted, or the battle
    /// is over.
    pub fn request(&self, side: SideRef) -> Option<String> {
        let i = side as usize;
        if self.ended || self.reqs[i].state == ReqState::Wait || self.answered[i].is_some() {
            return None;
        }
        Some(self.reqs[i].to_json())
    }

    /// `side`'s current request as PS's `emitRequest` sends it (`wait`
    /// requests included), or `None` once the battle is over (PS's
    /// `activeRequest` is null then).
    pub fn request_json(&self, side: SideRef) -> Option<String> {
        (!self.ended).then(|| self.reqs[side as usize].to_json())
    }

    /// The structured current request of `side` (wait included).
    pub fn side_request(&self, side: SideRef) -> &SideReq {
        &self.reqs[side as usize]
    }

    /// Stream chunks for `view` (`P1` / `P2` / `Spectator` / `Omniscient`).
    pub fn stream(&self, view: ProtocolView) -> &[String] {
        let k = VIEWS.iter().position(|v| *v == view).unwrap_or(0);
        &self.streams[k]
    }

    /// Answer `side`'s request with a PS choice string. `Err` carries the
    /// `|error|...` lines PS would send (they are also appended to the
    /// side's stream, followed by an updated `|request|` for an
    /// `[Unavailable choice]`).
    pub fn choose(&mut self, side: SideRef, input: &str) -> Result<(), Vec<String>> {
        let m = self.choose_detailed(side, input);
        if m.errors.is_empty() { Ok(()) } else { Err(m.errors) }
    }

    /// [`PsSession::choose`] with the updated request it sent, if any.
    pub fn choose_detailed(&mut self, side: SideRef, input: &str) -> SideMessages {
        let i = side as usize;
        if self.reqs[i].state != ReqState::Wait && !self.ended && self.cant_undo[i] {
            return self.side_out(i, SideMessages { errors: vec![CANT_UNDO.to_string()], updated_request: None });
        }
        let mut p = Parser::new(&self.reqs[i], side, self.ended);
        let res = p.choose(input);
        let ans = p.answer();
        let text = p.choice_text(&self.base);
        let cant_undo = p.cant_undo;
        let updated = p.updated.take();
        let errors = std::mem::take(&mut p.errors);
        let was_open = self.reqs[i].state != ReqState::Wait && !self.ended;
        let mut out = SideMessages { errors, updated_request: None };
        if let Some(r) = updated {
            self.reqs[i] = r;
            out.updated_request = Some(self.reqs[i].to_json());
        }
        match res {
            Err(()) => {
                // Side.choose cleared the old choice before failing.
                if was_open {
                    self.answered[i] = None;
                    self.choice_text[i] = None;
                    self.cant_undo[i] = cant_undo;
                }
                self.side_out(i, out)
            }
            Ok(()) => {
                self.answered[i] = Some(ans);
                self.choice_text[i] = Some(text);
                self.cant_undo[i] = cant_undo || !self.support_cancel;
                let waiting = (0..2).any(|k| self.reqs[k].state != ReqState::Wait && self.answered[k].is_none());
                if !waiting {
                    self.advance();
                }
                out
            }
        }
    }

    /// Record a side's messages in its stream and hand them back.
    fn side_out(&mut self, i: usize, m: SideMessages) -> SideMessages {
        for e in &m.errors {
            self.streams[i].push(e.clone());
        }
        if let Some(r) = &m.updated_request {
            self.streams[i].push(format!("|request|{r}"));
        }
        m
    }

    /// PS `>pN undo` (sim/battle.ts:3032-3058): drop `side`'s accepted
    /// choice so it can choose again.
    pub fn undo(&mut self, side: SideRef) -> SideMessages {
        let i = side as usize;
        if self.ended || self.reqs[i].state == ReqState::Wait {
            return SideMessages::default();
        }
        if self.cant_undo[i] {
            return self.side_out(i, SideMessages { errors: vec![CANT_UNDO.to_string()], updated_request: None });
        }
        // A move choice's mon reveals a maybeLocked / maybeDisabled flag
        // (updateDisabledRequest, sim/side.ts:851-903).
        let mut updated = false;
        if self.reqs[i].state == ReqState::Move {
            if let Some(Answer::Choices(cs)) = &self.answered[i] {
                let slots: Vec<u8> = cs
                    .iter()
                    .filter_map(|c| match c {
                        Choice::Move { actor_slot, .. } | Choice::Terastallize { actor_slot, .. } | Choice::MegaEvolve { actor_slot, .. } => Some(*actor_slot),
                        _ => None,
                    })
                    .collect();
                let doubles = self.reqs[i].n_active >= 2;
                for s in slots {
                    if let Some(Some(a)) = self.reqs[i].active.get_mut(s as usize) {
                        updated |= update_disabled_request(a, doubles);
                    }
                }
            }
        }
        self.answered[i] = None;
        self.choice_text[i] = None;
        let mut m = SideMessages::default();
        if updated {
            self.reqs[i].update = true;
            m.updated_request = Some(self.reqs[i].to_json());
        }
        self.side_out(i, m)
    }

    /// PS `>forcetie` (a turn-limit cutoff): `|tie`, and the battle ends.
    pub fn force_tie(&mut self) {
        self.force_win(None);
    }

    /// PS `battle.win(side)` (sim/battle.ts:1520-1543): `|`, `|win|name`
    /// (`|tie` for `None`), and the battle ends.
    pub fn force_win(&mut self, side: Option<SideRef>) {
        if self.ended {
            return;
        }
        self.base.protocol_force_win(side);
        let b = self.take_base();
        self.expose(&b, None);
        self.base = b;
        self.ended = true;
        for r in &mut self.reqs {
            r.state = ReqState::Wait;
        }
    }

    /// PS `>tiebreak` (sim/battle.ts:1467-1508): its message lines, then
    /// the win (a tie for `None`).
    pub fn tiebreak(&mut self, lines: &[String], winner: Option<SideRef>) {
        if self.ended {
            return;
        }
        self.base.protocol_add_lines(lines);
        self.force_win(winner);
    }

    /// PS `>reseed` (sim/battle.ts:360-363): the battle draws from `rng`
    /// from here on.
    pub fn reseed(&mut self, rng: crate::Rng) {
        self.base.rng = rng;
    }

    fn take_base(&mut self) -> Battle {
        std::mem::replace(&mut self.base, Battle::new(crate::battle::BattleConfig { format: crate::Format::Singles, seed: 0 }, vec![], vec![]))
    }

    fn advance(&mut self) {
        let answers = [self.answered[0].take(), self.answered[1].take()];
        // commitChoices logs each side's choice (sim/battle.ts:3009-3012).
        for k in 0..2 {
            if let Some(t) = self.choice_text[k].take().filter(|t| !t.is_empty()) {
                self.input_log.push(format!(">p{} {t}", k + 1));
            }
        }
        self.cant_undo = [false; 2];
        match self.stop.kind {
            SyncKind::TeamPreview => {
                let picks: [Vec<u8>; 2] = std::array::from_fn(|k| match &answers[k] {
                    Some(Answer::Team(t)) => t.clone(),
                    _ => Vec::new(),
                });
                let mut b = (self.builder)(Some(&picks));
                b.decision_phases = true;
                let k0 = 1;
                self.settle(b, k0, None);
            }
            SyncKind::Move | SyncKind::Replace(_) => {
                for k in 0..2 {
                    self.main[k] = match &answers[k] {
                        Some(Answer::Choices(c)) => c.clone(),
                        _ => Vec::new(),
                    };
                    self.mid[k].clear();
                }
                self.mid_answered = 0;
                self.run();
            }
            SyncKind::MidTurn(_) => {
                for k in 0..2 {
                    if let Some(Answer::Choices(c)) = &answers[k] {
                        self.mid[k].extend(c.iter().copied().filter(|c| matches!(c, Choice::Switch { .. })));
                    }
                }
                self.mid_answered += 1;
                self.run();
            }
        }
    }

    /// Replay the turn from `base` with the choices so far.
    fn run(&mut self) {
        let k0 = self.base.protocol_log().map(|l| l.sync_points().len()).unwrap_or(0);
        let mut b = self.base.clone_with_log();
        let p: [Vec<Choice>; 2] = std::array::from_fn(|k| {
            let mut v = self.main[k].clone();
            v.extend(self.mid[k].iter().copied());
            v
        });
        let r = b.step(&p[0], &p[1]);
        self.settle(b, k0, Some(r));
    }

    /// Find the stop `b` reached past its first `k0` stops and expose up
    /// to it; commit `b` once the turn is through.
    fn settle(&mut self, b: Battle, k0: usize, r: Option<StepResult>) {
        let stops: Vec<SyncPoint> = b.protocol_log().map(|l| l.sync_points()[k0.min(l.sync_points().len())..].to_vec()).unwrap_or_default();
        let mids: Vec<&SyncPoint> = stops.iter().filter(|s| matches!(s.kind, SyncKind::MidTurn(_))).collect();
        if r.is_some() && mids.len() > self.mid_answered {
            // PS is still mid-turn, waiting on this switch request.
            let sp = mids[self.mid_answered].clone();
            self.expose(&b, Some(sp.line));
            self.reqs = (*sp.info).clone();
            self.stop = sp;
            self.answered = [None, None];
            self.new_requests = true;
            return;
        }
        // The turn is through: commit.
        let last = stops.iter().rev().find(|s| !matches!(s.kind, SyncKind::MidTurn(_))).cloned();
        let ended = matches!(r, Some(StepResult::Ended { .. })) || b.protocol_log().is_some_and(|l| l.is_ended());
        match last {
            Some(sp) if !ended => {
                self.expose(&b, Some(sp.line));
                self.reqs = (*sp.info).clone();
                self.stop = sp;
                self.new_requests = true;
            }
            _ => {
                self.expose(&b, None);
                self.ended = true;
                for r in &mut self.reqs {
                    r.state = ReqState::Wait;
                }
            }
        }
        self.base = b;
        self.answered = [None, None];
        self.main = [Vec::new(), Vec::new()];
        self.mid = [Vec::new(), Vec::new()];
        self.mid_answered = 0;
    }
}

/// PS `updateDisabledRequest` (sim/side.ts:851-903) on one active slot:
/// reveal `maybeLocked` and (in doubles) `maybeDisabled`. True when the
/// request changed.
fn update_disabled_request(a: &mut crate::ps_request::ActiveRequest, doubles: bool) -> bool {
    let mut updated = false;
    if a.maybe_locked {
        a.maybe_locked = false;
        updated = true;
    }
    if a.maybe_disabled && doubles {
        a.maybe_disabled = false;
        updated = true;
        for m in a.moves.iter_mut() {
            if m.really_disabled {
                m.disabled = true;
            }
        }
    }
    if a.moves.iter().all(|m| m.disabled || m.id == "struggle") {
        if a.can_mega == Some(true) {
            a.can_mega = Some(false);
            updated = true;
        }
        if a.can_tera.is_some() {
            a.can_tera = None;
            updated = true;
        }
    }
    updated
}

// ---------------------------------------------------------------------------
// Choice parsing: a port of Side.choose (sim/side.ts:1192-1300) and the
// choose* helpers it calls, against the request the side was sent.
// ---------------------------------------------------------------------------

/// PS `toID`.
pub(crate) fn to_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// JS `parseInt(s)` (base 10): `None` for NaN.
fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let v: i64 = digits.parse().unwrap_or(i64::MAX);
    Some(if neg { -v } else { v })
}

/// JS `Utils.splitFirst(str, ' ')`.
fn split_first(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

/// `sim/dex-moves.ts` CHOOSABLE_TARGETS.
fn choosable(t: &str) -> bool {
    matches!(t, "normal" | "any" | "adjacentAlly" | "adjacentAllyOrSelf" | "adjacentFoe")
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Move { slot: u8, move_slot: u8, loc: i8, mega: bool, tera: bool, id: &'static str, locked: bool },
    Switch { slot: u8, pos: usize },
    Pass,
    Team { pos: usize },
}

/// One `choose()` call's state (PS `side.choice`).
struct Parser<'a> {
    req: SideReq,
    orig: &'a SideReq,
    side: SideRef,
    ended: bool,
    acts: Vec<Act>,
    switch_ins: Vec<usize>,
    forced_switches_left: usize,
    forced_passes_left: usize,
    mega: bool,
    tera: bool,
    error: bool,
    errors: Vec<String>,
    /// The request after an `[Unavailable choice]` update.
    updated: Option<SideReq>,
    /// PS `side.choice.cantUndo`.
    cant_undo: bool,
}

impl<'a> Parser<'a> {
    fn new(req: &'a SideReq, side: SideRef, ended: bool) -> Self {
        let mut p = Self {
            req: req.clone(),
            orig: req,
            side,
            ended,
            acts: Vec::new(),
            switch_ins: Vec::new(),
            forced_switches_left: 0,
            forced_passes_left: 0,
            mega: false,
            tera: false,
            error: false,
            errors: Vec::new(),
            updated: None,
            cant_undo: false,
        };
        p.clear();
        p
    }

    fn n_active(&self) -> usize {
        self.req.n_active as usize
    }

    fn game_type(&self) -> &'static str {
        if self.n_active() >= 2 { "doubles" } else { "singles" }
    }

    /// PS `clearChoice` (sim/side.ts:1123-1145).
    fn clear(&mut self) {
        self.acts.clear();
        self.switch_ins.clear();
        self.mega = false;
        self.tera = false;
        self.error = false;
        self.cant_undo = false;
        self.forced_switches_left = 0;
        self.forced_passes_left = 0;
        if self.req.state == ReqState::Switch {
            let out = self.req.force.iter().filter(|&&f| f).count();
            let can_in = self.req.mons.iter().skip(self.n_active()).filter(|m| !m.fainted).count();
            self.forced_switches_left = out.min(can_in);
            self.forced_passes_left = out - self.forced_switches_left;
        }
    }

    fn err(&mut self, msg: impl AsRef<str>) -> Result<(), ()> {
        self.error = true;
        self.errors.push(format!("|error|[Invalid choice] {}", msg.as_ref()));
        Err(())
    }

    fn unavailable(&mut self, msg: impl AsRef<str>) -> Result<(), ()> {
        self.error = true;
        self.errors.push(format!("|error|[Unavailable choice] {}", msg.as_ref()));
        self.req.update = true;
        self.updated = Some(self.req.clone());
        Err(())
    }

    fn name(&self, index: usize) -> String {
        self.req.mons.get(index).map(|m| m.name.clone()).unwrap_or_default()
    }

    /// PS `getChoiceIndex` with its auto-passes (sim/side.ts:1302-1325).
    fn choice_index(&mut self, is_pass: bool) -> usize {
        let mut index = self.acts.len();
        if !is_pass {
            match self.req.state {
                ReqState::Move => {
                    while index < self.n_active() && self.req.auto_pass.get(index).copied().unwrap_or(true) {
                        self.acts.push(Act::Pass);
                        index += 1;
                    }
                }
                ReqState::Switch => {
                    while index < self.n_active() && !self.req.force.get(index).copied().unwrap_or(false) {
                        self.acts.push(Act::Pass);
                        index += 1;
                    }
                }
                _ => {}
            }
        }
        index
    }

    /// PS `isChoiceDone` (sim/side.ts:538-550).
    fn done(&mut self) -> bool {
        match self.req.state {
            ReqState::Wait => true,
            _ if self.forced_switches_left > 0 => false,
            ReqState::TeamPreview => self.acts.len() >= self.picked_team_size(),
            _ => {
                self.choice_index(false);
                self.acts.len() >= self.n_active()
            }
        }
    }

    fn picked_team_size(&self) -> usize {
        let n = self.req.mons.len();
        if self.req.picked > 0 { n.min(self.req.picked as usize) } else { n }
    }

    /// `Battle.choose` + `Side.choose` (sim/battle.ts:2964-2980,
    /// sim/side.ts:1192-1300).
    fn choose(&mut self, input: &str) -> Result<(), ()> {
        if self.req.state == ReqState::Wait {
            return self.err(if self.ended {
                "Can't do anything: The game is over"
            } else {
                "Can't do anything: It's not your turn"
            });
        }
        self.clear();
        let strings: Vec<&str> = if input.starts_with("team ") { vec![input] } else { input.split(',').collect() };
        if strings.len() > self.n_active() {
            let gt = self.game_type();
            return self.err(format!(
                "Can't make choices: You sent choices for {} Pokémon, but this is a {gt} game!",
                strings.len()
            ));
        }
        let mut ok = true;
        for cs in strings {
            let (ty, data) = split_first(cs.trim());
            let mut data = data.trim().to_string();
            let mut ty = ty.to_string();
            if ty == "testfight" {
                ty = "move".into();
                data = "testfight".into();
            }
            let r = match ty.as_str() {
                "move" => self.parse_move(&data),
                "switch" => self.choose_switch(Some(&data)),
                "shift" => {
                    if !data.is_empty() {
                        self.err(format!("Unrecognized data after \"shift\": {data}"))
                    } else {
                        self.choose_shift()
                    }
                }
                "team" => self.choose_team(Some(&data)),
                "pass" | "skip" => {
                    if !data.is_empty() {
                        self.err(format!("Unrecognized data after \"pass\": {data}"))
                    } else {
                        self.choose_pass().map(|_| ())
                    }
                }
                "auto" | "default" => self.auto_choose(),
                _ => {
                    // No early return: PS keeps parsing the remaining parts.
                    let _ = self.err(format!("Unrecognized choice: {cs}"));
                    Ok(())
                }
            };
            if r.is_err() {
                ok = false;
                break;
            }
        }
        if !ok || self.error {
            if self.errors.is_empty() {
                return self.err(format!(
                    "Unknown error for choice: {input}. If you're not using a custom client, please report this as a bug."
                ));
            }
            return Err(());
        }
        if !self.done() {
            return self.err(format!("Incomplete choice: {input} - missing other pokemon"));
        }
        Ok(())
    }

    /// The `move` argument loop (sim/side.ts:1218-1272).
    fn parse_move(&mut self, data: &str) -> Result<(), ()> {
        let original = data.to_string();
        let mut data = data.to_string();
        let mut loc: Option<i8> = None;
        let mut event = "";
        loop {
            let bytes = data.as_bytes();
            let n = bytes.len();
            // /\s(?:-|\+)?[1-3]$/, except Conversion 2.
            let target_suffix = n >= 2
                && (b'1'..=b'3').contains(&bytes[n - 1])
                && (bytes[n - 2].is_ascii_whitespace()
                    || (n >= 3 && matches!(bytes[n - 2], b'-' | b'+') && bytes[n - 3].is_ascii_whitespace()));
            if target_suffix && to_id(&data) != "conversion2" {
                if loc.is_some() {
                    return self.err(format!("Conflicting arguments for \"move\": {original}"));
                }
                // parseInt(data.slice(-2)): "-2", "+2" or " 2".
                let tail = &data[n - 2..];
                loc = js_parse_int(tail).map(|v| v as i8);
                data = data[..n - 2].trim().to_string();
                continue;
            }
            let mut matched = false;
            for (suffix, ev) in [
                (" mega", "mega"),
                (" megax", "megax"),
                (" megay", "megay"),
                (" zmove", "zmove"),
                (" ultra", "ultra"),
                (" dynamax", "dynamax"),
                (" gigantamax", "dynamax"),
                (" max", "dynamax"),
                (" terastal", "terastallize"),
                (" terastallize", "terastallize"),
            ] {
                if data.ends_with(suffix) {
                    if !event.is_empty() {
                        return self.err(format!("Conflicting arguments for \"move\": {original}"));
                    }
                    event = ev;
                    data.truncate(data.len() - suffix.len());
                    matched = true;
                    break;
                }
            }
            if !matched {
                break;
            }
        }
        self.choose_move(&data, loc.unwrap_or(0), event)
    }

    /// PS `validTargetLoc` (sim/battle.ts:1527-1556) in doubles / singles.
    fn valid_target_loc(&self, loc: i8, source_slot: usize, target: &str) -> bool {
        if loc == 0 {
            return true;
        }
        let num = self.n_active() as i8;
        let source_loc = -(source_slot as i8 + 1);
        if loc.abs() > num {
            return false;
        }
        let is_self = source_loc == loc;
        let is_foe = loc > 0;
        let across = -(num + 1 - loc);
        let adjacent = if loc > 0 { (across - source_loc).abs() <= 1 } else { (loc - source_loc).abs() == 1 };
        match target {
            "randomNormal" | "scripted" | "normal" => adjacent,
            "adjacentAlly" => adjacent && !is_foe,
            "adjacentAllyOrSelf" => (adjacent && !is_foe) || is_self,
            "adjacentFoe" => adjacent && is_foe,
            "any" => !is_self,
            _ => false,
        }
    }

    /// PS `chooseMove` (sim/side.ts:552-830).
    fn choose_move(&mut self, move_text: &str, mut loc: i8, event: &str) -> Result<(), ()> {
        if self.req.state != ReqState::Move {
            let st = self.req.state.ps_name();
            return self.err(format!("Can't move: You need a {st} response"));
        }
        let index = self.choice_index(false);
        if index >= self.n_active() {
            return self.err("Can't move: You sent more choices than unfainted Pokémon.");
        }
        let name = self.name(index);
        let Some(Some(request)) = self.req.active.get(index).cloned() else {
            return self.err(format!("Can't move: Your {name} doesn't have a move 1"));
        };
        let auto = move_text.is_empty();
        let text = if auto { "1" } else { move_text };
        let mut move_idx: Option<usize> = None;
        let mut moveid: String;
        let mut target_type = String::new();
        if text.bytes().all(|b| b.is_ascii_digit()) {
            let n: i64 = text.parse().unwrap_or(i64::MAX);
            let i = n - 1;
            if i < 0 || i as usize >= request.moves.len() {
                return self.err(format!("Can't move: Your {name} doesn't have a move {}", i + 1));
            }
            move_idx = Some(i as usize);
            moveid = request.moves[i as usize].id.to_string();
            target_type = request.moves[i as usize].target.unwrap_or("").to_string();
        } else {
            moveid = to_id(text);
            if moveid.starts_with("hiddenpower") {
                moveid = "hiddenpower".into();
            }
            if let Some(i) = request.moves.iter().position(|m| m.id == moveid) {
                move_idx = Some(i);
                target_type = request.moves[i].target.unwrap_or("normal").to_string();
            }
            if target_type.is_empty() && moveid != "testfight" {
                return self.err(format!("Can't move: Your {name} doesn't have a move matching {moveid}"));
            }
        }
        if auto {
            // The first selectable move (hidden-disabled ones skipped).
            if let Some(i) = request.moves.iter().position(|m| !m.disabled && !m.really_disabled) {
                move_idx = Some(i);
                moveid = request.moves[i].id.to_string();
                target_type = request.moves[i].target.unwrap_or("").to_string();
            }
        }
        let move_name = move_idx.map(|i| request.moves[i].name).unwrap_or("");
        if event == "zmove" {
            return self.err(format!("Can't move: {name} can't use {move_name} as a Z-move"));
        }
        if event == "dynamax" {
            // getMaxMove exists for every move, so the target check runs on
            // the Max Move's target first (sim/side.ts:650-657).
            target_type = if request.moves.get(move_idx.unwrap_or(0)).is_some_and(|m| m.target == Some("self")) {
                "self".into()
            } else {
                "adjacentFoe".into()
            };
        }
        if auto || moveid == "testfight" {
            loc = 0;
        } else if choosable(&target_type) {
            if loc == 0 && self.n_active() >= 2 {
                return self.err(format!("Can't move: {move_name} needs a target"));
            }
            if !self.valid_target_loc(loc, index, &target_type) {
                return self.err(format!("Can't move: Invalid target for {move_name}"));
            }
        } else if loc != 0 {
            return self.err(format!("Can't move: You can't choose a target for {move_name}"));
        }
        let Some(mi) = move_idx else {
            // testfight on a mon whose Fight button is safe.
            return self.err(format!("Can't move: {name}'s Fight button is known to be safe"));
        };
        let mv = request.moves[mi].clone();
        if request.locked {
            // Locked move / Recharge / Struggle: PS queues it as is
            // (sim/side.ts:676-709).
            if request.maybe_locked {
                self.cant_undo = true;
            }
            self.acts.push(Act::Move { slot: index as u8, move_slot: mv.slot, loc: 0, mega: false, tera: false, id: mv.id, locked: true });
            return Ok(());
        }
        if mv.really_disabled {
            // chooseMove's disabled branch (sim/side.ts:731-760): the request
            // gets this move marked disabled with an (empty) disabledSource.
            let m = name.clone();
            if let Some(Some(a)) = self.req.active.get_mut(index) {
                if a.maybe_locked {
                    a.maybe_locked = false;
                }
                if a.maybe_disabled && self.req.n_active >= 2 {
                    a.maybe_disabled = false;
                    for x in a.moves.iter_mut() {
                        if x.really_disabled {
                            x.disabled = true;
                        }
                    }
                }
                if a.moves.iter().all(|x| x.disabled || x.id == "struggle") {
                    if a.can_mega == Some(true) {
                        a.can_mega = Some(false);
                    }
                    a.can_tera = None;
                }
                if let Some(x) = a.moves.get_mut(mi) {
                    x.disabled = true;
                    x.disabled_source_sent = true;
                }
            }
            return self.unavailable(format!("Can't move: {m}'s {move_name} is disabled"));
        }
        let mega = event == "mega";
        if mega && request.can_mega != Some(true) {
            return self.err(format!("Can't move: {name} can't mega evolve"));
        }
        if event == "megax" {
            return self.err(format!("Can't move: {name} can't mega evolve X"));
        }
        if event == "megay" {
            return self.err(format!("Can't move: {name} can't mega evolve Y"));
        }
        if mega && self.mega {
            return self.err("Can't move: You can only mega-evolve once per battle");
        }
        if event == "ultra" {
            return self.err(format!("Can't move: {name} can't ultra burst"));
        }
        if event == "dynamax" {
            return self.err("Can't move: Dynamaxing doesn't outside of Gen 8.");
        }
        let tera = event == "terastallize";
        if tera && request.can_tera.is_none() {
            return self.err(format!("Can't move: {name} can't Terastallize."));
        }
        if tera && self.tera {
            return self.err("Can't move: You can only Terastallize once per battle.");
        }
        self.acts.push(Act::Move { slot: index as u8, move_slot: mv.slot, loc, mega, tera, id: mv.id, locked: false });
        // sim/side.ts:835-840.
        if request.maybe_disabled && self.n_active() < 2 {
            self.cant_undo = true;
        }
        self.mega |= mega;
        self.tera |= tera;
        Ok(())
    }

    /// PS `chooseSwitch` (sim/side.ts:913-1029).
    fn choose_switch(&mut self, slot_text: Option<&str>) -> Result<(), ()> {
        if !matches!(self.req.state, ReqState::Move | ReqState::Switch) {
            let st = self.req.state.ps_name();
            return self.err(format!("Can't switch: You need a {st} response"));
        }
        let index = self.choice_index(false);
        if index >= self.n_active() {
            if self.req.state == ReqState::Switch {
                return self.err("Can't switch: You sent more switches than Pokémon that need to switch");
            }
            return self.err("Can't switch: You sent more choices than unfainted Pokémon");
        }
        let reviving = self.req.reviving.get(index).copied().unwrap_or(false);
        let n = self.n_active();
        let slot: i64 = match slot_text.filter(|s| !s.is_empty()) {
            None => {
                if self.req.state != ReqState::Switch {
                    return self.err("Can't switch: You need to select a Pokémon to switch in");
                }
                if reviving {
                    self.req.mons.iter().position(|m| m.fainted).unwrap_or(self.req.mons.len()) as i64
                } else {
                    if self.forced_switches_left == 0 {
                        return self.choose_pass().map(|_| ());
                    }
                    let mut s = n;
                    while s < self.req.mons.len() && (self.switch_ins.contains(&s) || self.req.mons[s].fainted) {
                        s += 1;
                    }
                    s as i64
                }
            }
            Some(t) => match js_parse_int(t).map(|v| v - 1) {
                Some(v) if v >= 0 => v,
                _ => {
                    let lower = t.to_lowercase();
                    let id = to_id(t);
                    match self.req.mons.iter().position(|m| m.name.to_lowercase() == lower || m.species_id == id) {
                        Some(i) => i as i64,
                        None => return self.err(format!("Can't switch: You do not have a Pokémon named \"{t}\" to switch to")),
                    }
                }
            },
        };
        let slot = slot as usize;
        if slot >= self.req.mons.len() {
            return self.err(format!("Can't switch: You do not have a Pokémon in slot {} to switch to", slot + 1));
        } else if slot < n && !reviving {
            return self.err("Can't switch: You can't switch to an active Pokémon");
        } else if self.switch_ins.contains(&slot) {
            return self.err(format!("Can't switch: The Pokémon in slot {} can only switch in once", slot + 1));
        }
        let target_fainted = self.req.mons[slot].fainted;
        if reviving {
            if !target_fainted {
                return self.err("Can't switch: You have to pass to a fainted Pokémon");
            }
            self.forced_switches_left = self.forced_switches_left.saturating_sub(1);
            self.acts.push(Act::Switch { slot: index as u8, pos: slot });
            return Ok(());
        }
        if target_fainted {
            return self.err("Can't switch: You can't switch to a fainted Pokémon");
        }
        if self.req.state == ReqState::Move {
            let trapped = self.req.active.get(index).and_then(|a| a.as_ref()).is_some_and(|a| a.really_trapped);
            if trapped {
                let a = self.req.active[index].as_mut().unwrap();
                let mut updated = false;
                if a.maybe_trapped {
                    a.maybe_trapped = false;
                    updated = true;
                }
                if !a.trapped {
                    a.trapped = true;
                    a.trapped_appended = true;
                    updated = true;
                }
                if updated {
                    return self.unavailable("Can't switch: The active Pokémon is trapped");
                }
                return self.err("Can't switch: The active Pokémon is trapped");
            }
            // sim/side.ts:998-1000.
            if self.req.active.get(index).and_then(|a| a.as_ref()).is_some_and(|a| a.maybe_trapped) {
                self.cant_undo = true;
            }
        } else if self.req.state == ReqState::Switch {
            self.forced_switches_left = self.forced_switches_left.saturating_sub(1);
        }
        self.switch_ins.push(slot);
        self.acts.push(Act::Switch { slot: index as u8, pos: slot });
        Ok(())
    }

    /// PS `chooseShift` (sim/side.ts:1097-1115).
    fn choose_shift(&mut self) -> Result<(), ()> {
        let index = self.choice_index(false);
        if index >= self.n_active() {
            return self.err(format!("Can't shift: You do not have a Pokémon in slot {}", index + 1));
        } else if self.req.state != ReqState::Move {
            return self.err("Can't shift: You can only shift during a move phase");
        }
        self.err("Can't shift: You can only shift to the center in triples")
    }

    /// PS `choosePass` (sim/side.ts:1327-1355): `Ok(false)` is PS's silent
    /// `return false`.
    fn choose_pass(&mut self) -> Result<bool, ()> {
        let index = self.choice_index(true);
        if index >= self.n_active() {
            self.error = true;
            return Err(());
        }
        let name = self.name(index);
        match self.req.state {
            ReqState::Switch => {
                if self.req.force.get(index).copied().unwrap_or(false) {
                    if self.forced_passes_left == 0 {
                        return self.err(format!("Can't pass: You need to switch in a Pokémon to replace {name}")).map(|_| false);
                    }
                    self.forced_passes_left -= 1;
                }
            }
            ReqState::Move => {
                if !self.req.auto_pass.get(index).copied().unwrap_or(true) {
                    return self.err(format!("Can't pass: Your {name} must make a move (or switch)")).map(|_| false);
                }
            }
            _ => return self.err("Can't pass: Not a move or switch request").map(|_| false),
        }
        self.acts.push(Act::Pass);
        Ok(true)
    }

    /// PS `chooseTeam` (sim/side.ts:1031-1095).
    fn choose_team(&mut self, data: Option<&str>) -> Result<(), ()> {
        if self.req.state != ReqState::TeamPreview {
            return self.err("Can't choose for Team Preview: You're not in a Team Preview phase");
        }
        let n_mons = self.req.mons.len();
        let picked = self.picked_team_size();
        let data = data.filter(|d| !d.is_empty());
        let mut bracketed = false;
        let mut positions: Vec<Option<i64>> = match data {
            None => (0..n_mons as i64).map(Some).collect(),
            Some(d) => {
                let mut t = d;
                if d.starts_with('[') && d.ends_with(']') && d.len() >= 2 {
                    bracketed = true;
                    t = d[1..d.len() - 1].trim();
                }
                let parts: Vec<String> = if bracketed || t.contains(',') || n_mons >= 10 {
                    t.split(',').map(str::to_string).collect()
                } else {
                    t.chars().map(|c| c.to_string()).collect()
                };
                parts.iter().map(|p| js_parse_int(p).map(|v| v - 1)).collect()
            }
        };
        if !bracketed {
            positions.truncate(picked);
        }
        if positions.len() < picked {
            for i in 0..picked as i64 {
                if !positions.contains(&Some(i)) {
                    positions.push(Some(i));
                }
                if positions.len() >= picked {
                    break;
                }
            }
        }
        if positions.len() != picked {
            return self.err(format!("Can't choose for Team Preview: You must choose exactly {picked} Pokémon"));
        }
        for (index, pos) in positions.iter().enumerate() {
            match pos {
                Some(p) if *p >= 0 && (*p as usize) < n_mons => {
                    if positions.iter().position(|q| q == pos) != Some(index) {
                        return self.err(format!("Can't choose for Team Preview: The Pokémon in slot {} can only switch in once", p + 1));
                    }
                }
                Some(p) => return self.err(format!("Can't choose for Team Preview: You do not have a Pokémon in slot {}", p + 1)),
                None => return self.err("Can't choose for Team Preview: You do not have a Pokémon in slot NaN"),
            }
        }
        for p in positions.into_iter().flatten() {
            self.switch_ins.push(p as usize);
            self.acts.push(Act::Team { pos: p as usize });
        }
        Ok(())
    }

    /// PS `autoChoose` (sim/side.ts:1357-1378).
    fn auto_choose(&mut self) -> Result<(), ()> {
        match self.req.state {
            ReqState::TeamPreview => {
                if !self.done() {
                    self.choose_team(None)?;
                }
            }
            ReqState::Switch => {
                let mut i = 0;
                while !self.done() {
                    self.choose_switch(None)?;
                    i += 1;
                    if i > 10 {
                        return Err(());
                    }
                }
            }
            ReqState::Move => {
                let mut i = 0;
                while !self.done() {
                    self.choose_move("", 0, "")?;
                    i += 1;
                    if i > 10 {
                        return Err(());
                    }
                }
            }
            ReqState::Wait => {}
        }
        Ok(())
    }

    /// The choice as PS's `side.getChoice()` prints it for the input log
    /// (sim/side.ts:323-351). A locked move keeps its stored target
    /// (sim/side.ts:676-688), which `b` (the turn's start) knows.
    fn choice_text(&self, b: &Battle) -> String {
        let teams: Vec<usize> = self.acts.iter().filter_map(|a| if let Act::Team { pos } = a { Some(*pos) } else { None }).collect();
        if teams.len() > 1 && teams.len() == self.acts.len() {
            return format!("team {}", teams.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", "));
        }
        let doubles = self.n_active() > 1;
        let parts: Vec<String> = self
            .acts
            .iter()
            .map(|a| match *a {
                Act::Move { slot, loc, mega, tera, id, locked, .. } => {
                    if locked && id == "struggle" {
                        return "move struggle".to_string();
                    }
                    let loc = if locked { locked_target_loc(b, self.side, slot) } else { loc };
                    let mut d = String::new();
                    if loc != 0 && doubles {
                        d.push_str(&format!(" {}{loc}", if loc > 0 { "+" } else { "" }));
                    }
                    if mega {
                        d.push_str(" mega");
                    }
                    if tera {
                        d.push_str(" terastallize");
                    }
                    format!("move {id}{d}")
                }
                Act::Switch { pos, .. } => format!("switch {}", pos + 1),
                Act::Team { pos } => format!("team {}", pos + 1),
                Act::Pass => "pass".to_string(),
            })
            .collect();
        parts.join(", ")
    }

    /// The accepted choice as engine choices / team picks.
    fn answer(&self) -> Answer {
        if self.req.state == ReqState::TeamPreview {
            return Answer::Team(
                self.acts
                    .iter()
                    .filter_map(|a| match a {
                        Act::Team { pos } => self.orig.mons.get(*pos).map(|m| m.team_idx),
                        _ => None,
                    })
                    .collect(),
            );
        }
        let foe = self.side.opposing();
        let mut out = Vec::with_capacity(self.acts.len());
        for (slot, a) in self.acts.iter().enumerate() {
            let actor_slot = slot as u8;
            out.push(match *a {
                Act::Move { slot, move_slot, loc, mega, tera, .. } => {
                    let target = match loc {
                        l if l > 0 => Some(Target { side: foe, slot: (l - 1) as u8 }),
                        l if l < 0 => Some(Target { side: self.side, slot: (-l - 1) as u8 }),
                        _ => None,
                    };
                    let move_slot = if move_slot == STRUGGLE_MOVE_SLOT { STRUGGLE_MOVE_SLOT } else { move_slot };
                    if mega {
                        Choice::MegaEvolve { actor_slot: slot, move_slot, target }
                    } else if tera {
                        Choice::Terastallize { actor_slot: slot, move_slot, target }
                    } else {
                        Choice::Move { actor_slot: slot, move_slot, target }
                    }
                }
                Act::Switch { slot, pos } => Choice::Switch { actor_slot: slot, team_index: self.orig.mons[pos].team_idx },
                Act::Pass | Act::Team { .. } => Choice::Pass { actor_slot },
            });
        }
        Answer::Choices(out)
    }
}

/// PS `lockedMoveTargetLoc` (sim/side.ts:677-681): the mon's
/// `lastMoveTargetLoc`, relative to it (foes positive, its side negative).
fn locked_target_loc(b: &Battle, side: SideRef, slot: u8) -> i8 {
    let Some(m) = b.side(side).active_mon(slot as usize) else { return 0 };
    let enc = m.last_used_move_target;
    if enc == 255 {
        return 0;
    }
    let (tside, tslot) = if enc < 2 { (SideRef::P1, enc) } else { (SideRef::P2, enc - 2) };
    if tside == side { -(tslot as i8 + 1) } else { tslot as i8 + 1 }
}
