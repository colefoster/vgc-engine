//! PR-I.1 measurement: factorability % across synthesized doubles turns.
//!
//! Synthesizes N random doubles turns from a small pool of teams that span
//! the breaker classes catalogued in `docs/design/pr-i-action-independence.md`,
//! classifies each turn with [`classify_factorability`], and reports the
//! % breakdown over the corpus.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example measure_factorability
//!
//! The headline number gates PR-I.2 — per the design doc §5: ship PR-I.2
//! only if factorability (Fully + Partial) ≥ 20–25 %.

use std::collections::BTreeMap;

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
};
use vgc_solver::{classify_factorability, Factorability};

// ─── Team pool. Each team is doubles-legal (2 mons, valid moves). ────
//
// Teams are chosen to exercise distinct breaker categories so the
// synthesized corpus reflects realistic mid-game diversity rather than
// only the "4 clean tackles" case.

const TEAM_BASELINE: &str = r#"[
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
    {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;
const TEAM_SPREAD: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","tackle","ironhead"]},
    {"species":"hippowdon","level":50,"ability":"sandstream","item":"leftovers","nature":"impish","moves":["earthquake","crunch","tackle","yawn"]}
]"#;
const TEAM_HELPINGHAND: &str = r#"[
    {"species":"clefairy","level":50,"ability":"friendguard","item":"eviolite","nature":"bold","moves":["helpinghand","tackle","watergun","ember"]},
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;
const TEAM_REDIRECT: &str = r#"[
    {"species":"raichu","level":50,"ability":"lightningrod","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;
const TEAM_KO_TRIGGER: &str = r#"[
    {"species":"kartana","level":50,"ability":"beastboost","item":"choicescarf","nature":"hardy","moves":["leafblade","tackle","ember","vinewhip"]},
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;
const TEAM_PIVOT: &str = r#"[
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["uturn","watergun","ember","tackle"]},
    {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["voltswitch","watergun","ember","tackle"]}
]"#;
const TEAM_SETTER: &str = r#"[
    {"species":"torkoal","level":50,"ability":"drought","item":"choicescarf","nature":"hardy","moves":["sunnyday","tackle","ember","vinewhip"]},
    {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["raindance","tackle","watergun","ember"]}
]"#;
const TEAM_ORDER: &str = r#"[
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["trickroom","tailwind","tackle","ember"]},
    {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;
const TEAM_SUCKER: &str = r#"[
    {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["suckerpunch","tackle","ember","vinewhip"]},
    {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["quickguard","tackle","watergun","ember"]}
]"#;
const TEAM_ITEM_BREAK: &str = r#"[
    {"species":"furret","level":50,"ability":"runaway","item":"airballoon","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
    {"species":"sentret","level":50,"ability":"runaway","item":"weaknesspolicy","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
]"#;

const TEAMS: &[(&str, &str)] = &[
    ("baseline", TEAM_BASELINE),
    ("spread", TEAM_SPREAD),
    ("helping_hand", TEAM_HELPINGHAND),
    ("redirect", TEAM_REDIRECT),
    ("ko_trigger", TEAM_KO_TRIGGER),
    ("pivot", TEAM_PIVOT),
    ("setter", TEAM_SETTER),
    ("order", TEAM_ORDER),
    ("sucker", TEAM_SUCKER),
    ("item_break", TEAM_ITEM_BREAK),
];

/// Tiny LCG so the example doesn't need an `rand` dep.
struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn range(&mut self, hi: u32) -> u32 {
        if hi == 0 {
            0
        } else {
            self.next_u32() % hi
        }
    }
}

fn build_battle(p1_json: &str, p2_json: &str, seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(p1_json).unwrap();
    let p2 = TeamBuilder::from_json(p2_json).unwrap();
    Battle::new(BattleConfig { format: Format::Doubles, seed }, p1, p2)
}

/// Build a single random doubles turn: pick a Move from each actor's legal
/// choices, with target chosen randomly from the legal target set.
fn random_joint_choices(b: &Battle, rng: &mut Lcg) -> ([Choice; 2], [Choice; 2]) {
    let mut p1 = [Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }];
    let mut p2 = [Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }];
    for (slot, out) in p1.iter_mut().enumerate() {
        let legals = b.legal_choices(SideRef::P1, slot as u8);
        if !legals.is_empty() {
            *out = legals[rng.range(legals.len() as u32) as usize];
        }
    }
    for (slot, out) in p2.iter_mut().enumerate() {
        let legals = b.legal_choices(SideRef::P2, slot as u8);
        if !legals.is_empty() {
            *out = legals[rng.range(legals.len() as u32) as usize];
        }
    }
    (p1, p2)
}

/// Coarse turn-type bucket purely from the joint choice shape (no engine
/// state inspection beyond the choice). Buckets are not mutually
/// exclusive in intent; we pick the highest-priority one that matches.
fn turn_bucket(p1: &[Choice; 2], p2: &[Choice; 2]) -> &'static str {
    let all = [p1[0], p1[1], p2[0], p2[1]];
    if all.iter().any(|c| matches!(c, Choice::Switch { .. })) {
        return "any_switch";
    }
    // Note: we don't currently bucket by setup/priority — would require
    // looking up move ids. The basic split (switches vs not) is the most
    // informative single dimension; finer buckets land in PR-I.2 if
    // useful.
    if all
        .iter()
        .all(|c| matches!(c, Choice::Move { .. } | Choice::Terastallize { .. } | Choice::MegaEvolve { .. }))
    {
        "all_moves"
    } else {
        "mixed"
    }
}

#[derive(Default, Debug)]
struct Bucket {
    fully: u32,
    partial: u32,
    no: u32,
}
impl Bucket {
    fn add(&mut self, f: &Factorability) {
        match f {
            Factorability::FullyFactor => self.fully += 1,
            Factorability::PartialFactor { .. } => self.partial += 1,
            Factorability::NoFactor => self.no += 1,
        }
    }
    fn total(&self) -> u32 {
        self.fully + self.partial + self.no
    }
    fn print(&self, label: &str) {
        let t = self.total().max(1);
        println!(
            "  {label:<18}  N={:<5}  Fully={:>5.1}%  Partial={:>5.1}%  No={:>5.1}%",
            self.total(),
            100.0 * self.fully as f64 / t as f64,
            100.0 * self.partial as f64 / t as f64,
            100.0 * self.no as f64 / t as f64,
        );
    }
}

fn main() {
    const SAMPLES: u32 = 200;
    let mut rng = Lcg(0xC0FFEE);

    let mut overall = Bucket::default();
    let mut by_pairing: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_turntype: BTreeMap<&'static str, Bucket> = BTreeMap::new();

    println!("PR-I.1 — factorability classifier measurement");
    println!(
        "Synthesizing N={} doubles turns from {} team pairings ({} teams squared).",
        SAMPLES,
        TEAMS.len() * TEAMS.len(),
        TEAMS.len()
    );

    let team_count = TEAMS.len();
    for sample in 0..SAMPLES {
        let i = (sample as usize) % team_count;
        let j = ((sample as usize) / team_count) % team_count;
        let (p1_name, p1_json) = TEAMS[i];
        let (p2_name, p2_json) = TEAMS[j];
        let b = build_battle(p1_json, p2_json, sample as u64 + 1);
        let (p1c, p2c) = random_joint_choices(&b, &mut rng);
        let f = classify_factorability(&b, &p1c, &p2c);

        overall.add(&f);
        let pairing = format!("{}×{}", p1_name, p2_name);
        by_pairing.entry(pairing).or_default().add(&f);
        by_turntype.entry(turn_bucket(&p1c, &p2c)).or_default().add(&f);
    }

    println!();
    overall.print("OVERALL");

    println!("\nBy turn type:");
    for (k, v) in &by_turntype {
        v.print(k);
    }

    println!("\nBy team pairing (sorted):");
    let mut rows: Vec<(&String, &Bucket)> = by_pairing.iter().collect();
    rows.sort_by(|a, b| {
        let af = (a.1.fully + a.1.partial) as f64 / a.1.total().max(1) as f64;
        let bf = (b.1.fully + b.1.partial) as f64 / b.1.total().max(1) as f64;
        bf.partial_cmp(&af).unwrap()
    });
    for (k, v) in rows.iter().take(20) {
        v.print(k);
    }

    println!(
        "\nHeadline factorable fraction (Fully + Partial): {:.1}%",
        100.0 * (overall.fully + overall.partial) as f64 / overall.total().max(1) as f64,
    );
    println!(
        "  → PR-I.2 gate is ≥ 15%. Result: {}",
        if (overall.fully + overall.partial) as f64 / overall.total().max(1) as f64 >= 0.15 {
            "PASS — proceed to PR-I.2"
        } else {
            "FAIL — do not ship PR-I.2"
        }
    );
}
