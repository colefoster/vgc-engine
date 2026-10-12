//! Usage-statistics prior for [`crate::position`]: per species, the
//! probability of each item, ability, move and spread.
//!
//! Embedded priors have stable ids (`data/smogon-stats/README.md` has the
//! provenance):
//!
//! * `smogon:2026-09/gen9championsvgc2026regmc-1500`: Smogon's Reg M-C chaos
//!   stats for 2026-09 at the 1500 cut (mimikyu's usage convention), trimmed
//!   by `scripts/trim_smogon_chaos.py`. Spreads are Champions stat points.
//! * `smogon:2026-08/gen9championsvgc2026regmb-0`: Reg M-B, 2026-08, cut 0.
//! * `bringfour:2026-09/regmc-reveals`: Bring Four's Reg M-C replay reveal
//!   shares (reveal-biased, no spreads).
//!
//! `a+b` chains ids: species `a` lacks come from `b`. `"default"` picks by
//! format at [`Prior::for_format`]: Reg M-B formats get [`DEFAULT_REGMB`]
//! (= [`V0_4_0_DEFAULT`], 0.4.0's default), every other format
//! [`DEFAULT_REGMC`].
//!
//! Also loadable: a Bring Four / chaos-derived JSON file of the same shape
//! (a `cutoffs` map takes cutoff `0`, else the lowest), or a Smogon moveset
//! `.txt` (parsed by [`crate::smogon_stats`]).
//!
//! Probabilities are the stats' percentages / 100. Moves are "share of sets
//! with the move" (they sum to about 4).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::smogon_stats;

/// `(id, text)` of every embedded prior file.
const EMBEDDED: [(&str, &str); 3] = [
    ("smogon:2026-09/gen9championsvgc2026regmc-1500", include_str!("../../../data/smogon-stats/2026-09/gen9championsvgc2026regmc-1500.json")),
    ("smogon:2026-08/gen9championsvgc2026regmb-0", include_str!("../../../data/smogon-stats/2026-08/gen9championsvgc2026regmb-0.json")),
    ("bringfour:2026-09/regmc-reveals", include_str!("../../../data/smogon-stats/2026-09/bringfour-regmc-reveals.json")),
];

/// The default for Reg M-C (and any non-M-B format). The rare species under
/// the 0.1% usage trim fall back to Bring Four's reveal shares.
pub const DEFAULT_REGMC: &str = "smogon:2026-09/gen9championsvgc2026regmc-1500+bringfour:2026-09/regmc-reveals";
/// The default for Reg M-B formats: 0.4.0's default, unchanged. The reveal
/// fallback covers Reg M-C teams played under M-B rules (the conformance
/// harness does that); without it their moves match half as often.
pub const DEFAULT_REGMB: &str = V0_4_0_DEFAULT;
/// vgc-engine 0.4.0's default, for every format.
pub const V0_4_0_DEFAULT: &str = "smogon:2026-08/gen9championsvgc2026regmb-0+bringfour:2026-09/regmc-reveals";

/// One spread: nature slug, six stat values, probability. `sp` = the values
/// are Champions stat points (0-32), else EVs.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorSpread {
    pub nature: String,
    pub values: [u16; 6],
    pub sp: bool,
    pub p: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpeciesPrior {
    /// Share of teams with the species (0-1); 0 when the source has none.
    pub usage: f64,
    /// `(slug, p)`, most likely first.
    pub items: Vec<(String, f64)>,
    pub abilities: Vec<(String, f64)>,
    pub moves: Vec<(String, f64)>,
    pub spreads: Vec<PriorSpread>,
}

#[derive(Debug, Clone, Default)]
pub struct Prior {
    /// Its id: an embedded id or `a+b` chain, a path, `none`, or `default`
    /// (unresolved: see [`Prior::for_format`]).
    pub source: String,
    /// SHA-256 (hex) of the file content; a chain's are `+`-joined in id
    /// order. Empty for `none` and `default`.
    pub sha256: String,
    /// `default`: resolved per format.
    auto: bool,
    species: HashMap<String, SpeciesPrior>,
    /// Consulted for species this prior lacks.
    fallback: Option<Box<Prior>>,
}

pub fn slug(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

fn pairs(v: Option<&Value>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = v
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let x = x.as_array()?;
                    let name = slug(x.first()?.as_str()?);
                    let p = x.get(1)?.as_f64()? / 100.0;
                    (!name.is_empty() && name != "other" && name != "nothing").then_some((name, p))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

impl Prior {
    /// No prior: every hidden field falls back to the documented defaults.
    pub fn none() -> Self {
        Prior { source: "none".into(), ..Default::default() }
    }

    /// The embedded default: resolved per format by [`Prior::for_format`].
    pub fn embedded() -> Self {
        Prior { source: "default".into(), auto: true, ..Default::default() }
    }

    /// The prior a replay of format `format_id` uses: the default's pick for
    /// the format (Reg M-B -> [`DEFAULT_REGMB`], else [`DEFAULT_REGMC`]),
    /// else `self`.
    pub fn for_format(&self, format_id: &str) -> &Prior {
        if !self.auto {
            return self;
        }
        static MB: OnceLock<Prior> = OnceLock::new();
        static MC: OnceLock<Prior> = OnceLock::new();
        if format_id.to_ascii_lowercase().contains("regmb") {
            MB.get_or_init(|| Self::load(DEFAULT_REGMB).expect("embedded prior"))
        } else {
            MC.get_or_init(|| Self::load(DEFAULT_REGMC).expect("embedded prior"))
        }
    }

    /// `"default"` / `""` = the per-format default, `"none"` = no prior, an
    /// embedded id or an `a+b` chain of ids / paths, else a file path
    /// (`.json` Bring Four / chaos-derived, else a Smogon moveset `.txt`).
    pub fn load(spec: &str) -> Result<Self, String> {
        match spec {
            "" | "default" | "embedded" => return Ok(Self::embedded()),
            "none" => return Ok(Self::none()),
            _ => {}
        }
        if let Some(text) = EMBEDDED.iter().find(|(id, _)| *id == spec).map(|x| x.1) {
            let mut p = Self::from_json_text(text)?;
            p.source = spec.to_string();
            p.sha256 = sha256_hex(text.as_bytes());
            return Ok(p);
        }
        if let Some((head, rest)) = spec.split_once('+').filter(|_| !std::path::Path::new(spec).exists()) {
            let mut p = Self::load(head)?;
            let f = Self::load(rest)?;
            if p.auto || f.auto || p.fallback.is_some() {
                return Err(format!("prior {spec}: only ids and paths chain"));
            }
            p.source = spec.to_string();
            p.sha256 = format!("{}+{}", p.sha256, f.sha256);
            p.fallback = Some(Box::new(f));
            return Ok(p);
        }
        let text = std::fs::read_to_string(spec).map_err(|e| format!("prior {spec}: {e} (embedded ids: {})", Self::embedded_ids().join(", ")))?;
        let mut p = if text.trim_start().starts_with('{') { Self::from_json_text(&text)? } else { Self::from_moveset_txt(&text)? };
        p.source = spec.to_string();
        p.sha256 = sha256_hex(text.as_bytes());
        Ok(p)
    }

    /// The embedded priors' ids.
    pub fn embedded_ids() -> Vec<&'static str> {
        EMBEDDED.iter().map(|x| x.0).collect()
    }

    pub fn from_json_text(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("prior json: {e}"))?;
        let block = if let Some(c) = v.get("cutoffs").and_then(Value::as_object) {
            let key = if c.contains_key("0") { "0".to_string() } else { c.keys().min_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX)).cloned().unwrap_or_default() };
            c.get(&key).cloned().unwrap_or(Value::Null)
        } else {
            v.clone()
        };
        let sp_obj = block.get("species").and_then(Value::as_object).ok_or("prior json: no `species` map")?;
        let mut species = HashMap::new();
        for (name, e) in sp_obj {
            let mut spreads = Vec::new();
            for s in e.get("spreads").and_then(Value::as_array).into_iter().flatten() {
                let nature = slug(s.get("nature").and_then(Value::as_str).unwrap_or(""));
                let (txt, sp) = match (s.get("stat_points").and_then(Value::as_str), s.get("evs").and_then(Value::as_str)) {
                    (Some(t), _) => (t, true),
                    (None, Some(t)) => (t, false),
                    _ => continue,
                };
                let nums: Vec<u16> = txt.split('/').filter_map(|x| x.trim().parse().ok()).collect();
                if nums.len() != 6 || nature.is_empty() {
                    continue;
                }
                let p = s.get("pct").and_then(Value::as_f64).unwrap_or(0.0) / 100.0;
                spreads.push(PriorSpread { nature, values: [nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]], sp, p });
            }
            spreads.sort_by(|a, b| b.p.total_cmp(&a.p));
            species.insert(
                slug(name),
                SpeciesPrior {
                    usage: e.get("usage_pct").and_then(Value::as_f64).unwrap_or(0.0) / 100.0,
                    items: pairs(e.get("items")),
                    abilities: pairs(e.get("abilities")),
                    moves: pairs(e.get("moves")),
                    spreads,
                },
            );
        }
        Ok(Prior { source: "json".into(), species, ..Default::default() })
    }

    /// A Smogon moveset `.txt`. Its spread numbers are Champions stat points
    /// in Champions formats (the parser's `n * 8` EVs are mapped back).
    pub fn from_moveset_txt(text: &str) -> Result<Self, String> {
        let st = smogon_stats::parse(text).map_err(|e| e.to_string())?;
        let total: f64 = st.species.iter().map(|s| s.raw_count as f64).sum::<f64>().max(1.0);
        let mut species = HashMap::new();
        for s in st.species {
            let f = |v: &[(String, f32)]| -> Vec<(String, f64)> { v.iter().filter(|(n, _)| n != "other" && n != "nothing").map(|(n, p)| (n.clone(), *p as f64 / 100.0)).collect() };
            let spreads = s
                .spreads
                .iter()
                .filter(|(n, e, _)| !(n == "serious" && e.hp == 0 && e.atk == 0 && e.spe == 0))
                .map(|(n, e, p)| {
                    let back = |x: u8| (x as u16 + 7) / 8;
                    PriorSpread { nature: n.clone(), values: [back(e.hp), back(e.atk), back(e.def), back(e.spa), back(e.spd), back(e.spe)], sp: true, p: *p as f64 / 100.0 }
                })
                .collect();
            species.insert(
                s.species.clone(),
                SpeciesPrior { usage: 4.0 * s.raw_count as f64 / total, items: f(&s.items), abilities: f(&s.abilities), moves: f(&s.moves), spreads },
            );
        }
        Ok(Prior { source: "txt".into(), species, ..Default::default() })
    }

    /// The prior for a species (display name or slug). A forme the stats
    /// don't list falls back to its base species (`Salamence-Mega` ->
    /// `Salamence`, `Indeedee-F` stays `indeedeef`).
    pub fn get(&self, species: &str) -> Option<&SpeciesPrior> {
        self.get_own(species).or_else(|| self.fallback.as_deref().and_then(|f| f.get(species)))
    }

    fn get_own(&self, species: &str) -> Option<&SpeciesPrior> {
        let s = slug(species);
        if let Some(p) = self.species.get(&s) {
            return Some(p);
        }
        for suf in ["mega", "megax", "megay", "megaz"] {
            if let Some(b) = s.strip_suffix(suf) {
                if let Some(p) = self.species.get(b) {
                    return Some(p);
                }
            }
        }
        let base = species.split('-').next().map(slug).unwrap_or_default();
        self.species.get(&base)
    }

    /// The stats keys a mon shown as `species` may be listed under: the
    /// species itself and its Mega formes (stats list a stone holder under
    /// its Mega forme, `Charizard-Mega-Y`). `(key slug, usage)`.
    pub fn keys_for(&self, species: &str) -> Vec<(String, f64)> {
        let s = slug(species);
        let mega = format!("{s}mega");
        let mut out: Vec<(String, f64)> = self.species.iter().filter(|(k, _)| **k == s || k.starts_with(&mega)).map(|(k, v)| (k.clone(), v.usage)).collect();
        if out.is_empty() {
            if let Some(f) = &self.fallback {
                return f.keys_for(species);
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    pub fn is_empty(&self) -> bool {
        !self.auto && self.species.is_empty() && self.fallback.as_ref().is_none_or(|f| f.is_empty())
    }
}

/// The deterministic draw behind `prior_sample_seed` (SplitMix64): one per
/// mon, so a mon's sample does not depend on the order mons are resolved.
#[derive(Debug, Clone)]
pub struct PriorSampler(u64);

impl PriorSampler {
    pub fn new(seed: u64) -> Self {
        PriorSampler(seed)
    }

    /// The sampler for team member `index` of `side`.
    pub fn for_mon(seed: u64, side: usize, index: usize) -> Self {
        let mut s = PriorSampler(seed);
        let mix = s.next_u64() ^ ((side as u64) << 32 | index as u64);
        PriorSampler(mix)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// An index drawn with probability proportional to `weights`
    /// (uniform when they sum to 0); `None` when empty.
    pub fn pick(&mut self, weights: &[f64]) -> Option<usize> {
        if weights.is_empty() {
            return None;
        }
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
        if total <= 0.0 {
            return Some(((u * weights.len() as f64) as usize).min(weights.len() - 1));
        }
        let mut x = u * total;
        for (k, w) in weights.iter().enumerate() {
            x -= w.max(0.0);
            if x < 0.0 {
                return Some(k);
            }
        }
        weights.iter().rposition(|w| *w > 0.0)
    }
}

/// SHA-256 of `data`, lowercase hex (FIPS 180-4), so a report's hash matches
/// `shasum -a 256` of the prior file without a crypto dependency.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn default_resolves_by_format() {
        let d = Prior::embedded();
        assert!(!d.is_empty());
        let mc = d.for_format("gen9championsvgc2026regmc");
        assert_eq!(mc.source, DEFAULT_REGMC);
        assert_eq!(mc.sha256.split('+').count(), 2);
        let k = mc.get("Kingambit").expect("kingambit");
        assert!(k.usage > 0.05);
        assert_eq!(k.abilities[0].0, "defiant");
        assert!(k.spreads[0].sp && k.spreads[0].values.iter().all(|&v| v <= 32));
        assert!(mc.get("Rillaboom").is_some_and(|r| !r.spreads.is_empty()), "M-C stats carry Rillaboom's spreads");
        assert!(mc.get("Salamence-Mega").is_some());
        assert!(mc.get("Charizard-Mega-Y").is_some());
        assert_eq!(d.for_format("gen9championsvgc2026regmb").source, DEFAULT_REGMB);
        // An explicit prior is never swapped.
        let mb = Prior::load("smogon:2026-08/gen9championsvgc2026regmb-0").unwrap();
        assert_eq!(mb.for_format("gen9championsvgc2026regmc").source, "smogon:2026-08/gen9championsvgc2026regmb-0");
        assert!(mb.get("Rillaboom").is_none(), "no fallback without a chain");
    }

    #[test]
    fn v0_4_0_default_reproduces() {
        let p = Prior::load(V0_4_0_DEFAULT).unwrap();
        assert_eq!(p.source, V0_4_0_DEFAULT);
        let k = p.get("Kingambit").expect("kingambit");
        assert!(k.usage > 0.1);
        assert_eq!(k.abilities[0].0, "defiant");
        assert!(k.spreads[0].sp);
        assert!(p.get("Salamence-Mega").is_some(), "Reg M-C fallback");
        assert!(p.get("Rillaboom").is_some_and(|r| r.moves[0].0 == "fakeout" && r.spreads.is_empty()));
        assert_eq!(p.sha256, format!("{}+{}", Prior::load("smogon:2026-08/gen9championsvgc2026regmb-0").unwrap().sha256, Prior::load("bringfour:2026-09/regmc-reveals").unwrap().sha256));
        assert!(Prior::load("smogon:2026-07/nope").is_err());
    }
}
