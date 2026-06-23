//! PS-conformance differential harness.
//!
//! Drives a battle in Pokémon Showdown first (the `tools/ps-golden-driver`
//! conformance mode), capturing every randomized outcome under a *semantic*
//! key plus the resolved choice sequence and per-turn state. We then replay
//! the same choices into vgc-engine with an [`Rng::OracleKeyed`] table built
//! from those outcomes, so RNG is neutralized WITHOUT mirroring PS's PRNG
//! stream — any remaining per-turn state divergence is a real mechanic bug.
//!
//! The cross-language key/representation contract lives in
//! `docs/conformance-key-contract.md`; this crate is its Rust half. The two
//! representation flips the runner is responsible for (and PS/the driver are
//! NOT) are applied in [`event_for_draw`]:
//!   * damage bucket: `engine = 15 - ps_random16`
//!   * accuracy/secondary percent: `engine = ps_random100 + 1`

use std::collections::{HashMap, VecDeque};

use serde::Deserialize;
use vgc_engine_core::data;
use vgc_engine_core::rng::{Rng, RngDecision, RngEvent, RngKey, SlotRef, NO_SLOT};
use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, StepResult, Target, TeamBuilder,
};

// ---------------------------------------------------------------------------
// Input schema (mirrors the conformance driver's JSON; see contract doc).
// ---------------------------------------------------------------------------

/// One PS-driven battle: legal teams, the seed PS ran under (informational;
/// the engine replays outcomes, not the LCG), and the per-turn record.
#[derive(Debug, Deserialize)]
pub struct PsBattle {
    pub format: String,
    #[serde(default)]
    pub seed: Vec<u16>,
    /// PS export ("Pokepaste") text for each side.
    pub p1team: String,
    pub p2team: String,
    pub turns: Vec<TurnRecord>,
}

#[derive(Debug, Deserialize)]
pub struct TurnRecord {
    pub turn: u32,
    pub choices: SideChoices,
    #[serde(default)]
    pub draws: Vec<DrawRecord>,
    /// Post-turn state keyed by slot ref ("p1a", "p2b", …).
    #[serde(default)]
    pub state: HashMap<String, MonState>,
}

#[derive(Debug, Deserialize)]
pub struct SideChoices {
    #[serde(default)]
    pub p1: Vec<String>,
    #[serde(default)]
    pub p2: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MonState {
    pub hp: u16,
    #[serde(default)]
    pub maxhp: u16,
    #[serde(default)]
    pub fainted: bool,
}

/// One recorded randomized outcome. `value` is the RAW PS value (bool for
/// crit; 0..15 for damage; 0..99 roll or bool for accuracy/secondary; etc.);
/// the runner applies the contract's representation flips.
#[derive(Debug, Deserialize)]
pub struct DrawRecord {
    pub turn: u32,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(rename = "move", default)]
    pub move_slug: Option<String>,
    pub decision: String,
    pub value: serde_json::Value,
    /// True when accuracy/secondary `value` is a pass/fail bool rather than
    /// the underlying 0..99 roll (PS sometimes only exposes the bool).
    #[serde(default)]
    pub raw_is_bool: bool,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub turn: u32,
    pub slot: String,
    pub field: &'static str,
    pub engine: i64,
    pub ps: i64,
}

#[derive(Debug, Clone)]
pub struct BattleReport {
    /// Turns that matched PS exactly before the first divergence (or the end).
    pub matched_turns: u32,
    /// The earliest divergence, if any (downstream cascades are not reported).
    pub divergence: Option<Divergence>,
    /// Keyed draws that missed the table and fell back (health metric).
    pub unmatched_draws: u32,
    /// Slugs that didn't resolve to an engine move id (dropped from the table).
    pub unresolved_moves: Vec<String>,
}

impl BattleReport {
    pub fn is_clean(&self) -> bool {
        self.divergence.is_none() && self.unmatched_draws == 0
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers (the error-prone, unit-tested core)
// ---------------------------------------------------------------------------

/// `"p1a" -> 0, "p1b" -> 1, "p2a" -> 2, "p2b" -> 3` (`side*2 + slot`).
pub fn parse_slot_ref(s: &str) -> Option<SlotRef> {
    let b = s.as_bytes();
    if b.len() != 3 || b[0] != b'p' {
        return None;
    }
    let side = match b[1] {
        b'1' => 0u8,
        b'2' => 2,
        _ => return None,
    };
    let slot = match b[2] {
        b'a' => 0u8,
        b'b' => 1,
        _ => return None,
    };
    Some(side + slot)
}

fn decode_slot_ref(s: &str) -> Option<(SideRef, usize)> {
    let r = parse_slot_ref(s)?;
    let side = if r < 2 { SideRef::P1 } else { SideRef::P2 };
    Some((side, (r % 2) as usize))
}

fn decision_of(s: &str) -> Option<RngDecision> {
    Some(match s {
        "accuracy" => RngDecision::Accuracy,
        "crit" => RngDecision::Crit,
        "damage" => RngDecision::Damage,
        "secondary" => RngDecision::Secondary,
        "range" => RngDecision::Range,
        "tiebreak" => RngDecision::Tiebreak,
        _ => return None,
    })
}

/// Convert one recorded draw to the engine-convention [`RngEvent`], applying
/// the contract's representation flips. Returns `None` if the value shape
/// doesn't match the decision (logged as a skipped draw by the caller).
pub fn event_for_draw(d: &DrawRecord) -> Option<RngEvent> {
    match d.decision.as_str() {
        "crit" => Some(RngEvent::Crit(d.value.as_bool()?)),
        // PS `random(16)`: 0 = max-roll (100%), 15 = min (85%). Engine bucket
        // is the opposite convention, so flip.
        "damage" => {
            let r = u8::try_from(d.value.as_u64()?).ok()?;
            Some(RngEvent::DamageRoll(15u8.saturating_sub(r)))
        }
        // Engine checks `roll <= threshold` (roll 1..=100); PS checks
        // `random(100) < threshold` (roll 0..=99). `+1` aligns the two. With
        // only a bool, synthesize a value that reproduces the same pass/fail:
        // pass -> 1 (<= any threshold >= 1), fail -> 100 (> any threshold < 100).
        "accuracy" | "secondary" => {
            if d.raw_is_bool {
                Some(RngEvent::PercentRoll(if d.value.as_bool()? { 1 } else { 100 }))
            } else {
                let r = u8::try_from(d.value.as_u64()?.min(99)).ok()?;
                Some(RngEvent::PercentRoll(r.saturating_add(1)))
            }
        }
        "range" => Some(RngEvent::Range(u32::try_from(d.value.as_u64()?).ok()?)),
        "tiebreak" => Some(RngEvent::Tiebreak(d.value.as_u64()?)),
        _ => None,
    }
}

/// Build the keyed-oracle table from every draw in the battle. Move slugs are
/// resolved to numeric engine move ids here (the engine keys on the id it
/// already holds). Unresolved slugs are dropped and reported.
pub fn build_table(
    battle: &PsBattle,
) -> (HashMap<RngKey, VecDeque<RngEvent>>, Vec<String>) {
    let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
    let mut unresolved: Vec<String> = Vec::new();
    for turn in &battle.turns {
        for d in &turn.draws {
            let Some(decision) = decision_of(&d.decision) else {
                continue;
            };
            let Some(event) = event_for_draw(d) else {
                continue;
            };
            let actor = d
                .actor
                .as_deref()
                .and_then(parse_slot_ref)
                .unwrap_or(NO_SLOT);
            let target = d
                .target
                .as_deref()
                .and_then(parse_slot_ref)
                .unwrap_or(NO_SLOT);
            let move_id = match d.move_slug.as_deref() {
                Some(slug) => match move_id_of(slug) {
                    Some(id) => id,
                    None => {
                        if !unresolved.iter().any(|u| u == slug) {
                            unresolved.push(slug.to_string());
                        }
                        continue;
                    }
                },
                None => 0,
            };
            let key = RngKey {
                turn: d.turn,
                actor,
                target,
                move_id,
                decision,
            };
            table.entry(key).or_default().push_back(event);
        }
    }
    (table, unresolved)
}

/// Map a PS move slug to the engine's numeric move id (its index into
/// `data::MOVES`, which is exactly what `battle.rs` keys on).
fn move_id_of(slug: &str) -> Option<u16> {
    data::MOVES
        .iter()
        .position(|m| m.slug == slug)
        .and_then(|i| u16::try_from(i).ok())
}

/// Parse one PS choice string for the given actor slot/side into a [`Choice`].
/// Handles `move N`, `move N T` (T = PS positional target), `switch K`,
/// `pass`. `N`/`K` are 1-based in PS; we convert to 0-based.
pub fn parse_choice(s: &str, actor_slot: u8, actor_side: SideRef) -> Result<Choice, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    match parts.as_slice() {
        ["move", n] => Ok(Choice::Move {
            actor_slot,
            move_slot: parse_1based(n)?,
            target: None,
        }),
        ["move", n, t] => Ok(Choice::Move {
            actor_slot,
            move_slot: parse_1based(n)?,
            target: Some(parse_ps_target(t, actor_side)?),
        }),
        ["switch", k] => Ok(Choice::Switch {
            actor_slot,
            team_index: parse_1based(k)?,
        }),
        ["pass"] => Ok(Choice::Pass { actor_slot }),
        _ => Err(format!("unrecognized PS choice: {s:?}")),
    }
}

fn parse_1based(s: &str) -> Result<u8, String> {
    let n: u8 = s.parse().map_err(|_| format!("bad index {s:?}"))?;
    n.checked_sub(1).ok_or_else(|| format!("index {s:?} not 1-based"))
}

/// PS positional target: `1`/`2` = foe slots a/b, `-1`/`-2` = ally slots a/b.
fn parse_ps_target(t: &str, actor_side: SideRef) -> Result<Target, String> {
    let n: i8 = t.parse().map_err(|_| format!("bad target {t:?}"))?;
    let foe = match actor_side {
        SideRef::P1 => SideRef::P2,
        SideRef::P2 => SideRef::P1,
    };
    match n {
        1 | 2 => Ok(Target { side: foe, slot: (n as u8) - 1 }),
        -1 | -2 => Ok(Target { side: actor_side, slot: (-n as u8) - 1 }),
        _ => Err(format!("unsupported PS target {t:?}")),
    }
}

fn parse_side_choices(
    raw: &[String],
    side: SideRef,
) -> Result<Vec<Choice>, String> {
    raw.iter()
        .enumerate()
        .map(|(slot, s)| parse_choice(s, slot as u8, side))
        .collect()
}

// ---------------------------------------------------------------------------
// Replay + diff
// ---------------------------------------------------------------------------

/// Replay a PS-driven battle into the engine under keyed-outcome injection and
/// diff per-turn state. Stops reporting at the first divergence (downstream
/// cascades are noise — see the design doc's first-divergence isolation).
pub fn replay(battle: &PsBattle) -> Result<BattleReport, String> {
    let p1 = TeamBuilder::from_showdown_text(&battle.p1team)
        .map_err(|e| format!("p1 team: {e:?}"))?;
    let p2 = TeamBuilder::from_showdown_text(&battle.p2team)
        .map_err(|e| format!("p2 team: {e:?}"))?;
    let (table, unresolved) = build_table(battle);
    let format = if battle.format.contains("doubles") {
        Format::Doubles
    } else {
        Format::Singles
    };
    let rng = Rng::oracle_keyed(table, 0xC0FFEE);
    let mut b = Battle::with_rng(BattleConfig { format, seed: 0 }, rng, p1, p2);

    let mut matched_turns = 0u32;
    let mut divergence = None;
    'turns: for turn in &battle.turns {
        let p1c = parse_side_choices(&turn.choices.p1, SideRef::P1)?;
        let p2c = parse_side_choices(&turn.choices.p2, SideRef::P2)?;
        let result = b.step(&p1c, &p2c);

        for (slot_ref, expected) in &turn.state {
            let Some((side, slot)) = decode_slot_ref(slot_ref) else {
                return Err(format!("bad state slot ref {slot_ref:?}"));
            };
            let mon = match side {
                SideRef::P1 => b.p1.active_mon(slot),
                SideRef::P2 => b.p2.active_mon(slot),
            };
            let (eng_hp, eng_fainted) = match mon {
                Some(m) => (m.current_hp, m.fainted || m.current_hp == 0),
                None => (0, true),
            };
            if eng_hp != expected.hp {
                divergence = Some(Divergence {
                    turn: turn.turn,
                    slot: slot_ref.clone(),
                    field: "hp",
                    engine: eng_hp as i64,
                    ps: expected.hp as i64,
                });
                break 'turns;
            }
            if eng_fainted != expected.fainted {
                divergence = Some(Divergence {
                    turn: turn.turn,
                    slot: slot_ref.clone(),
                    field: "fainted",
                    engine: eng_fainted as i64,
                    ps: expected.fainted as i64,
                });
                break 'turns;
            }
        }
        matched_turns += 1;
        if matches!(result, StepResult::Ended { .. }) {
            break;
        }
    }

    let unmatched_draws = b.rng().unmatched_draws().unwrap_or(0);
    Ok(BattleReport {
        matched_turns,
        divergence,
        unmatched_draws,
        unresolved_moves: unresolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_ref_roundtrip() {
        assert_eq!(parse_slot_ref("p1a"), Some(0));
        assert_eq!(parse_slot_ref("p1b"), Some(1));
        assert_eq!(parse_slot_ref("p2a"), Some(2));
        assert_eq!(parse_slot_ref("p2b"), Some(3));
        assert_eq!(parse_slot_ref("p3a"), None);
        assert_eq!(parse_slot_ref("x1a"), None);
        assert_eq!(parse_slot_ref("p1"), None);
        assert_eq!(decode_slot_ref("p2b"), Some((SideRef::P2, 1)));
    }

    fn draw(decision: &str, value: serde_json::Value, raw_is_bool: bool) -> DrawRecord {
        DrawRecord {
            turn: 1,
            actor: Some("p1a".into()),
            target: Some("p2a".into()),
            move_slug: Some("crunch".into()),
            decision: decision.into(),
            value,
            raw_is_bool,
        }
    }

    #[test]
    fn damage_bucket_is_flipped() {
        // PS random(16)=0 is the MAX roll → engine bucket 15; PS 15 → 0.
        assert_eq!(event_for_draw(&draw("damage", 0.into(), false)), Some(RngEvent::DamageRoll(15)));
        assert_eq!(event_for_draw(&draw("damage", 15.into(), false)), Some(RngEvent::DamageRoll(0)));
        assert_eq!(event_for_draw(&draw("damage", 7.into(), false)), Some(RngEvent::DamageRoll(8)));
    }

    #[test]
    fn percent_roll_is_plus_one() {
        // PS random(100)=49 → engine percent 50 (engine checks <=, PS <).
        assert_eq!(event_for_draw(&draw("accuracy", 49.into(), false)), Some(RngEvent::PercentRoll(50)));
        assert_eq!(event_for_draw(&draw("secondary", 0.into(), false)), Some(RngEvent::PercentRoll(1)));
        assert_eq!(event_for_draw(&draw("accuracy", 99.into(), false)), Some(RngEvent::PercentRoll(100)));
    }

    #[test]
    fn percent_bool_synthesizes_passfail() {
        // pass -> 1 (always <= threshold); fail -> 100 (> any sub-100 threshold).
        assert_eq!(event_for_draw(&draw("secondary", true.into(), true)), Some(RngEvent::PercentRoll(1)));
        assert_eq!(event_for_draw(&draw("accuracy", false.into(), true)), Some(RngEvent::PercentRoll(100)));
    }

    #[test]
    fn crit_passthrough() {
        assert_eq!(event_for_draw(&draw("crit", true.into(), false)), Some(RngEvent::Crit(true)));
        assert_eq!(event_for_draw(&draw("crit", false.into(), false)), Some(RngEvent::Crit(false)));
    }

    #[test]
    fn move_slug_resolves_to_index() {
        // Crunch must resolve to a real engine move id; an invented slug must not.
        assert!(move_id_of("crunch").is_some());
        assert_eq!(move_id_of("notarealmove"), None);
        // The resolved id round-trips through the MOVES table by slug.
        let id = move_id_of("crunch").unwrap();
        assert_eq!(data::MOVES[id as usize].slug, "crunch");
    }

    #[test]
    fn build_table_groups_by_key_and_flips() {
        let b = PsBattle {
            format: "gen9customgame".into(),
            seed: vec![1, 2, 3, 4],
            p1team: String::new(),
            p2team: String::new(),
            turns: vec![TurnRecord {
                turn: 1,
                choices: SideChoices { p1: vec![], p2: vec![] },
                draws: vec![
                    draw("crit", true.into(), false),
                    draw("damage", 0.into(), false),
                ],
                state: HashMap::new(),
            }],
        };
        let (table, unresolved) = build_table(&b);
        assert!(unresolved.is_empty());
        let mv = move_id_of("crunch").unwrap();
        let crit_key = RngKey { turn: 1, actor: 0, target: 2, move_id: mv, decision: RngDecision::Crit };
        let dmg_key = RngKey { turn: 1, actor: 0, target: 2, move_id: mv, decision: RngDecision::Damage };
        assert_eq!(table.get(&crit_key).unwrap()[0], RngEvent::Crit(true));
        assert_eq!(table.get(&dmg_key).unwrap()[0], RngEvent::DamageRoll(15));
    }

    #[test]
    fn unresolved_move_is_reported_not_panicked() {
        let mut d = draw("crit", true.into(), false);
        d.move_slug = Some("definitelynotamove".into());
        let b = PsBattle {
            format: "gen9customgame".into(),
            seed: vec![],
            p1team: String::new(),
            p2team: String::new(),
            turns: vec![TurnRecord {
                turn: 1,
                choices: SideChoices { p1: vec![], p2: vec![] },
                draws: vec![d],
                state: HashMap::new(),
            }],
        };
        let (table, unresolved) = build_table(&b);
        assert!(table.is_empty());
        assert_eq!(unresolved, vec!["definitelynotamove".to_string()]);
    }

    #[test]
    fn parse_choice_forms() {
        assert_eq!(
            parse_choice("move 1", 0, SideRef::P1).unwrap(),
            Choice::Move { actor_slot: 0, move_slot: 0, target: None }
        );
        assert_eq!(
            parse_choice("switch 3", 0, SideRef::P1).unwrap(),
            Choice::Switch { actor_slot: 0, team_index: 2 }
        );
        // "move 2 1" — second move at the first FOE slot.
        assert_eq!(
            parse_choice("move 2 1", 1, SideRef::P1).unwrap(),
            Choice::Move { actor_slot: 1, move_slot: 1, target: Some(Target { side: SideRef::P2, slot: 0 }) }
        );
        // ally target (-2) for a P2 actor → own side, slot b.
        assert_eq!(
            parse_choice("move 1 -2", 0, SideRef::P2).unwrap(),
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(Target { side: SideRef::P2, slot: 1 }) }
        );
        assert!(parse_choice("move 0", 0, SideRef::P1).is_err()); // not 1-based
        assert!(parse_choice("nonsense", 0, SideRef::P1).is_err());
    }

    // PS-export teams for the replay tests (Houndoom uses Crunch on Blissey).
    const P1_TEAM: &str = "Houndoom\nAbility: Flash Fire\nLevel: 50\nHasty Nature\n- Crunch\n- Splash\n";
    const P2_TEAM: &str = "Blissey\nAbility: Natural Cure\nLevel: 50\nBold Nature\n- Splash\n";

    fn crunch_draws(crit: bool, dmg_ps: u8, secondary_ps: u8) -> Vec<DrawRecord> {
        let mk = |decision: &str, value: serde_json::Value| DrawRecord {
            turn: 1,
            actor: Some("p1a".into()),
            target: Some("p2a".into()),
            move_slug: Some("crunch".into()),
            decision: decision.into(),
            value,
            raw_is_bool: false,
        };
        vec![
            mk("accuracy", 0.into()),  // PS roll 0 → engine 1 → hit
            mk("crit", crit.into()),
            mk("damage", dmg_ps.into()),
            mk("secondary", secondary_ps.into()),
        ]
    }

    /// End-to-end: build a battle from PS-export teams, inject the four crunch
    /// draws, replay, and confirm every draw resolved from the table
    /// (unmatched == 0) and the run completed. Uses the engine as its own
    /// oracle — the REAL PS oracle arrives via the driver's sample fixture.
    #[test]
    fn replay_consumes_injected_draws_with_no_unmatched() {
        let battle = PsBattle {
            format: "gen9customgame".into(),
            seed: vec![1, 2, 3, 4],
            p1team: P1_TEAM.into(),
            p2team: P2_TEAM.into(),
            turns: vec![TurnRecord {
                turn: 1,
                choices: SideChoices {
                    p1: vec!["move 1".into()],
                    p2: vec!["move 1".into()],
                },
                // PS damage roll 0 = max; crit; secondary roll 0 = proc.
                draws: crunch_draws(true, 0, 0),
                state: HashMap::new(),
            }],
        };
        let report = replay(&battle).expect("replay ok");
        assert!(report.unresolved_moves.is_empty(), "crunch resolves");
        assert_eq!(report.matched_turns, 1, "the one turn ran");
        assert_eq!(
            report.unmatched_draws, 0,
            "all four crunch draws keyed and consumed from the table"
        );
    }

    /// The differ flags a deliberately wrong expected HP as a divergence at the
    /// right turn/slot — proving the state comparison is wired, not vacuous.
    #[test]
    fn replay_detects_wrong_expected_hp() {
        let mut state = HashMap::new();
        state.insert(
            "p2a".to_string(),
            MonState { hp: 60000, maxhp: 0, fainted: false }, // impossible HP
        );
        let battle = PsBattle {
            format: "gen9customgame".into(),
            seed: vec![1, 2, 3, 4],
            p1team: P1_TEAM.into(),
            p2team: P2_TEAM.into(),
            turns: vec![TurnRecord {
                turn: 1,
                choices: SideChoices {
                    p1: vec!["move 1".into()],
                    p2: vec!["move 1".into()],
                },
                draws: crunch_draws(false, 15, 99),
                state,
            }],
        };
        let report = replay(&battle).expect("replay ok");
        let d = report.divergence.expect("a divergence was reported");
        assert_eq!(d.turn, 1);
        assert_eq!(d.slot, "p2a");
        assert_eq!(d.field, "hp");
        assert_eq!(d.ps, 60000);
    }
}
