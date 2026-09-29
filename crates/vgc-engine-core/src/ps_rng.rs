//! Pokémon Showdown-compatible PRNG (`ps-rng` feature).
//!
//! A bit-exact port of PS's `sim/prng.ts` at smogon/pokemon-showdown
//! `a5df8274e85b0889bf2a9b3422a08b39732374fc` (2026-09-22):
//!
//! - `PRNG` — the high-level API (`random()`, `random(n)`, `random(m, n)`,
//!   `randomChance`, `sample`, `shuffle`). Every call draws exactly one
//!   32-bit value, including `random(1)` / `randomChance(1, 1)`.
//! - `SodiumRNG` — PS's default since 2024: ChaCha20 (IETF, 12-byte nonce
//!   `"LibsodiumDRG"`, counter 0) keyed by a 32-byte seed. Each `next()`
//!   generates 36 keystream bytes; bytes `0..32` become the next seed and
//!   bytes `32..36` (big-endian) are the output. Seed string
//!   `sodium,<hex>`; a 16-byte hex seed is right-padded with zeros.
//! - `Gen5RNG` — the legacy 64-bit LCG (`a = 0x5D588B656C078965`,
//!   `c = 0x269EC3`), output = upper 32 bits. Seed strings `gen5,<16 hex>`
//!   or `a,b,c,d` (four u16s, big-endian), the format old input logs use.
//!
//! This module only supplies the number stream. Which engine call sites draw
//! from it, and in what order, is the battle's job (see `Rng::Ps` in
//! `rng.rs` and the `ps-rng` gated blocks in `battle.rs` / `order.rs`).
//!
//! It also carries an optional **draw trace**: every draw appends
//! `(seq, turn, op, args, raw, result, engine call site)`. The conformance
//! crate's `ps-trace-diff` tool lines this up against the same trace captured
//! from PS (`tools/accuracy/ps-battle.js`) to find the first divergent draw.

use serde::{Deserialize, Serialize};

/// ChaCha20 quarter round on the working state.
#[inline]
fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// One ChaCha20 block (RFC 8439) for `key`, block `counter`, `nonce`.
fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[0] = 0x6170_7865;
    init[1] = 0x3320_646e;
    init[2] = 0x7962_2d32;
    init[3] = 0x6b20_6574;
    for i in 0..8 {
        init[4 + i] = u32::from_le_bytes([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] =
            u32::from_le_bytes([nonce[4 * i], nonce[4 * i + 1], nonce[4 * i + 2], nonce[4 * i + 3]]);
    }
    let mut s = init;
    for _ in 0..10 {
        qr(&mut s, 0, 4, 8, 12);
        qr(&mut s, 1, 5, 9, 13);
        qr(&mut s, 2, 6, 10, 14);
        qr(&mut s, 3, 7, 11, 15);
        qr(&mut s, 0, 5, 10, 15);
        qr(&mut s, 1, 6, 11, 12);
        qr(&mut s, 2, 7, 8, 13);
        qr(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// PS `SodiumRNG.NONCE` — `"LibsodiumDRG"`.
const SODIUM_NONCE: [u8; 12] = *b"LibsodiumDRG";

/// The 32-bit source behind [`PsRng`] (PS's `RNG` interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PsBackend {
    /// `Gen5RNG`: full 64-bit LCG state.
    Gen5(u64),
    /// `SodiumRNG`: the 32-byte ChaCha20 key that doubles as the seed.
    Sodium([u8; 32]),
}

impl PsBackend {
    #[inline]
    fn next(&mut self) -> u32 {
        match self {
            PsBackend::Gen5(state) => {
                *state = state
                    .wrapping_mul(0x5D58_8B65_6C07_8965)
                    .wrapping_add(0x0000_0000_0026_9EC3);
                (*state >> 32) as u32
            }
            PsBackend::Sodium(seed) => {
                let block = chacha20_block(seed, 0, &SODIUM_NONCE);
                seed.copy_from_slice(&block[..32]);
                u32::from_be_bytes([block[32], block[33], block[34], block[35]])
            }
        }
    }
}

/// Error from [`PsRng::from_seed_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedError(pub String);

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad PS seed: {}", self.0)
    }
}

/// One traced draw. `op` names the engine `Rng` method that drew, `a`/`b` its
/// PS-equivalent arguments (`random(a)` / `random(a, b)`; `0` when unused),
/// `raw` the 32-bit PRNG output and `result` what the call returned. `file` /
/// `line` are the engine call site (captured with `#[track_caller]`).
#[derive(Debug, Clone, Serialize)]
pub struct PsDraw {
    pub seq: u32,
    pub turn: u32,
    pub op: &'static str,
    pub a: u32,
    pub b: u32,
    pub raw: u32,
    pub result: u32,
    pub file: &'static str,
    pub line: u32,
    /// Move-resolution context the battle last set (`Rng::set_move_context`
    /// / `set_decision`): acting slot, engine move id, target slot and the
    /// Accuracy/Secondary tag. Stale between moves; informational only.
    pub actor: u8,
    pub move_id: u16,
    pub target: u8,
    pub decision: &'static str,
}

/// PS `PRNG`: a backend plus the optional draw trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsRng {
    backend: PsBackend,
    /// Number of 32-bit draws taken so far.
    draws: u32,
    /// Current trace turn label, set by the harness between steps.
    trace_turn: u32,
    #[serde(skip)]
    trace: Option<Vec<PsDraw>>,
    #[serde(skip)]
    ctx_actor: u8,
    #[serde(skip)]
    ctx_move: u16,
    #[serde(skip)]
    ctx_target: u8,
    #[serde(skip)]
    ctx_decision: &'static str,
}

impl PsRng {
    pub fn gen5(seed: [u16; 4]) -> Self {
        let state = ((seed[0] as u64) << 48)
            | ((seed[1] as u64) << 32)
            | ((seed[2] as u64) << 16)
            | (seed[3] as u64);
        Self::with_backend(PsBackend::Gen5(state))
    }

    /// `seed` is the raw 32-byte key (a 16-byte PS seed zero-padded).
    pub fn sodium(seed: [u8; 32]) -> Self {
        Self::with_backend(PsBackend::Sodium(seed))
    }

    fn with_backend(backend: PsBackend) -> Self {
        Self {
            backend,
            draws: 0,
            trace_turn: 0,
            trace: None,
            ctx_actor: 0xFF,
            ctx_move: 0,
            ctx_target: 0xFF,
            ctx_decision: "",
        }
    }

    /// Parse any seed string PS's `PRNG.setSeed` accepts:
    /// `sodium,<hex ≤64>`, `gen5,<16 hex>`, or `a,b,c,d`.
    pub fn from_seed_str(seed: &str) -> Result<Self, SeedError> {
        let err = || SeedError(seed.to_string());
        if let Some(hex) = seed.strip_prefix("sodium,") {
            if hex.len() > 64 || hex.len() % 2 != 0 {
                return Err(err());
            }
            let padded = format!("{hex:0<64}");
            let mut key = [0u8; 32];
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&padded[2 * i..2 * i + 2], 16).map_err(|_| err())?;
            }
            Ok(Self::sodium(key))
        } else if let Some(hex) = seed.strip_prefix("gen5,") {
            if hex.len() < 16 {
                return Err(err());
            }
            let mut q = [0u16; 4];
            for (i, v) in q.iter_mut().enumerate() {
                *v = u16::from_str_radix(&hex[4 * i..4 * i + 4], 16).map_err(|_| err())?;
            }
            Ok(Self::gen5(q))
        } else if seed.starts_with(|c: char| c.is_ascii_digit()) {
            let parts: Vec<&str> = seed.split(',').collect();
            if parts.len() != 4 {
                return Err(err());
            }
            let mut q = [0u16; 4];
            for (v, p) in q.iter_mut().zip(parts) {
                *v = p.trim().parse().map_err(|_| err())?;
            }
            Ok(Self::gen5(q))
        } else {
            Err(err())
        }
    }

    /// PS `getSeed()` string for the current state.
    pub fn seed_string(&self) -> String {
        match self.backend {
            PsBackend::Gen5(s) => format!(
                "{},{},{},{}",
                (s >> 48) as u16,
                (s >> 32) as u16,
                (s >> 16) as u16,
                s as u16
            ),
            PsBackend::Sodium(k) => {
                let mut out = String::from("sodium,");
                for b in k {
                    out.push_str(&format!("{b:02x}"));
                }
                out
            }
        }
    }

    pub fn draws(&self) -> u32 {
        self.draws
    }

    pub fn set_context(&mut self, actor: u8, move_id: u16, target: u8) {
        self.ctx_actor = actor;
        self.ctx_move = move_id;
        self.ctx_target = target;
    }

    pub fn set_decision_label(&mut self, d: &'static str) {
        self.ctx_decision = d;
    }

    pub fn enable_trace(&mut self) {
        self.trace = Some(Vec::new());
    }

    pub fn set_trace_turn(&mut self, turn: u32) {
        self.trace_turn = turn;
    }

    pub fn take_trace(&mut self) -> Option<Vec<PsDraw>> {
        self.trace.as_mut().map(std::mem::take)
    }

    #[inline]
    fn raw(&mut self) -> u32 {
        self.draws += 1;
        self.backend.next()
    }

    #[inline]
    fn log(&mut self, op: &'static str, a: u32, b: u32, raw: u32, result: u32, at: &'static std::panic::Location<'static>) {
        if let Some(t) = self.trace.as_mut() {
            t.push(PsDraw {
                seq: self.draws - 1,
                turn: self.trace_turn,
                op,
                a,
                b,
                raw,
                result,
                file: at.file(),
                line: at.line(),
                actor: self.ctx_actor,
                move_id: self.ctx_move,
                target: self.ctx_target,
                decision: self.ctx_decision,
            });
        }
    }

    /// PS `random(n)`: `floor(r * n / 2^32)`. Draws even for `n <= 1`.
    #[track_caller]
    pub fn random_n(&mut self, op: &'static str, n: u32) -> u32 {
        let r = self.raw();
        let v = ((r as u64 * n as u64) >> 32) as u32;
        self.log(op, n, 0, r, v, std::panic::Location::caller());
        v
    }

    /// PS `random(m, n)`: `floor(r * (n - m) / 2^32) + m`.
    #[track_caller]
    pub fn random_range(&mut self, op: &'static str, m: u32, n: u32) -> u32 {
        let r = self.raw();
        let v = ((r as u64 * (n - m) as u64) >> 32) as u32 + m;
        self.log(op, m, n, r, v, std::panic::Location::caller());
        v
    }

    /// PS `randomChance(num, den)`: `random(den) < num`.
    #[track_caller]
    pub fn random_chance(&mut self, op: &'static str, num: u32, den: u32) -> bool {
        let r = self.raw();
        let v = ((r as u64 * den as u64) >> 32) as u32;
        let pass = v < num;
        self.log(op, den, num, r, pass as u32, std::panic::Location::caller());
        pass
    }

    /// PS `random()` with no arguments; returns the raw 32-bit value (the
    /// float is `raw / 2^32`, so callers compare raws directly).
    #[track_caller]
    pub fn random_raw(&mut self, op: &'static str) -> u32 {
        let r = self.raw();
        self.log(op, 0, 0, r, r, std::panic::Location::caller());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference vectors from PS `sim/prng.ts` at a5df8274 (Node 26):
    /// `new PRNG(seed).rng.next()` ×8, then on a fresh PRNG
    /// `[random(16), random(100), random(2,5), randomChance(1,24), random(4), random(1)]`.
    #[test]
    fn sodium_matches_ps() {
        let mut p = PsRng::from_seed_str("sodium,0123456789abcdef0123456789abcdef").unwrap();
        let raws: Vec<u32> = (0..8).map(|_| p.raw()).collect();
        assert_eq!(
            raws,
            [1564598223, 2628916691, 3021397871, 1916167142, 4157841405, 1076669892, 1746556866, 1976461880]
        );
        let mut z = PsRng::from_seed_str(&format!("sodium,{}", "00".repeat(16))).unwrap();
        let raws: Vec<u32> = (0..8).map(|_| z.raw()).collect();
        assert_eq!(
            raws,
            [4267713560, 2597593732, 1237973749, 687352079, 630900176, 1586261390, 31191460, 2410399535]
        );
    }

    #[test]
    fn sodium_mixed_calls_and_seed_roundtrip() {
        let mut q = PsRng::from_seed_str("sodium,0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(q.random_n("t", 16), 5);
        assert_eq!(q.random_n("t", 100), 61);
        assert_eq!(q.random_range("t", 2, 5), 4);
        assert!(!q.random_chance("t", 1, 24));
        assert_eq!(q.random_n("t", 4), 3);
        assert_eq!(q.random_n("t", 1), 0);
        assert_eq!(
            q.seed_string(),
            "sodium,9190dbf6d492ab7e91153e435ae97e24632ace90e8b61a1bddf9b873b681f336"
        );
    }

    #[test]
    fn gen5_matches_ps_both_seed_formats() {
        for s in ["gen5,0001000200030004", "1,2,3,4"] {
            let mut p = PsRng::from_seed_str(s).unwrap();
            let raws: Vec<u32> = (0..8).map(|_| p.raw()).collect();
            assert_eq!(
                raws,
                [2030470262, 3793892072, 2851743046, 574702432, 3620926519, 2468984361, 4121665136, 1670872321]
            );
        }
        let mut q = PsRng::from_seed_str("43981,4660,22136,39612").unwrap();
        let mixed = [
            q.random_n("t", 16),
            q.random_n("t", 100),
            q.random_range("t", 2, 5),
            q.random_chance("t", 1, 24) as u32,
            q.random_n("t", 4),
            q.random_n("t", 1),
        ];
        assert_eq!(mixed, [3, 37, 2, 0, 0, 0]);
        assert_eq!(q.seed_string(), "15652,56935,47398,52930");
    }

    #[test]
    fn trace_records_call_site() {
        let mut p = PsRng::from_seed_str("1,2,3,4").unwrap();
        p.enable_trace();
        p.set_trace_turn(3);
        p.random_n("damage", 16);
        let t = p.take_trace().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].turn, t[0].op, t[0].a, t[0].result), (3, "damage", 16, 7));
        assert!(t[0].file.ends_with("ps_rng.rs"));
    }
}
