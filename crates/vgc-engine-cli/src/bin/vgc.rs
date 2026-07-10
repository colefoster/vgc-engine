//! `vgc` — the fast damage-calc CLI front end.
//!
//! Tracer bullet: `vgc calc chomp lando eq` parses terse attacker /
//! defender / move strings (with an alias table), runs the engine's
//! `calc`, and prints a human-readable damage range + KO estimate.
//!
//! This is a thin layer: parse args → [`QuickMon::parse`] → [`calc`] →
//! format. All calc logic lives in `vgc-engine-core::calc`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use vgc_engine_core::calc::{
    best_move, calc, matchup, min_evs_to_ko, min_evs_to_survive, speed_tier, AtkStat, DamageResult,
    DefStat, Field, KoChance, MoveDamage, QuickMon, SpeedContext, SpeedWinner,
};
use vgc_engine_core::{Terrain, Weather};

#[derive(Parser)]
#[command(name = "vgc", about = "vgc-engine fast damage calculator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Calculate damage: `vgc calc <attacker> <defender> <move>`.
    ///
    /// Attacker/defender accept the terse ` / `-delimited grammar, e.g.
    /// "Garchomp @ Life Orb / Jolly / 252 Atk". Bare species + common
    /// shorthand (chomp, lando, eq, cc, ...) resolve through the alias
    /// table.
    Calc {
        /// Attacker spec (terse string or alias, e.g. "chomp").
        attacker: String,
        /// Defender spec (terse string or alias, e.g. "lando").
        defender: String,
        /// Move (name or alias, e.g. "eq").
        move_: String,
        /// Weather: sun | rain | sand | snow.
        #[arg(long)]
        weather: Option<String>,
        /// Terrain: electric | grassy | psychic | misty.
        #[arg(long)]
        terrain: Option<String>,
        /// Treat as a Doubles spread hit (applies the ×0.75 modifier).
        #[arg(long)]
        spread: bool,
        /// Force a critical hit.
        #[arg(long)]
        crit: bool,
        /// Emit the full `DamageResult` as JSON instead of the
        /// human-readable block.
        #[arg(long)]
        json: bool,
    },

    /// Does the defender survive one hit? `vgc survives <atk> <def> <move>`.
    /// Prints "survives" / "does not survive" (+ the KO chance) and exits
    /// 0 iff it survives every roll.
    Survives {
        attacker: String,
        defender: String,
        move_: String,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        terrain: Option<String>,
        #[arg(long)]
        spread: bool,
    },

    /// Compare two mons' speed: `vgc outspeeds <a> <b>`. Respects tailwind,
    /// weather speed abilities, and Trick Room.
    Outspeeds {
        /// First mon.
        a: String,
        /// Second mon.
        b: String,
        /// Weather: sun | rain | sand | snow (Swift Swim / Chlorophyll / …).
        #[arg(long)]
        weather: Option<String>,
        /// `a` has tailwind up.
        #[arg(long)]
        tailwind_a: bool,
        /// `b` has tailwind up.
        #[arg(long)]
        tailwind_b: bool,
        /// Trick Room is active (slower moves first).
        #[arg(long)]
        trick_room: bool,
    },

    /// Print a mon's effective speed tier: `vgc speed <mon>`. Folds in
    /// boosts, paralysis, Choice Scarf / Iron Ball, Paradox Spe, Unburden,
    /// tailwind, and weather speed abilities.
    Speed {
        mon: String,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        tailwind: bool,
    },

    /// Pick the hardest-hitting move: `vgc best <atk> <def> <moves>` where
    /// `<moves>` is a comma-separated list (names or aliases).
    Best {
        attacker: String,
        defender: String,
        /// Comma-separated candidate moves, e.g. "eq,dragonclaw,poisonjab".
        moves: String,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        terrain: Option<String>,
        #[arg(long)]
        spread: bool,
    },

    /// Two-sided matchup summary: `vgc matchup <a> <a_moves> <b> <b_moves>`.
    /// Each `<*_moves>` is a comma-separated list. Reports each side's best
    /// move, who moves first, and the KO verdicts.
    Matchup {
        a: String,
        /// A's comma-separated moves.
        a_moves: String,
        b: String,
        /// B's comma-separated moves.
        b_moves: String,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        terrain: Option<String>,
        #[arg(long)]
        spread: bool,
        /// Trick Room active (slower moves first).
        #[arg(long)]
        trick_room: bool,
    },

    /// Min EVs for the defender to survive one hit:
    /// `vgc survive-evs <atk> <def> <move>`. `--stat` picks which
    /// defensive stat to invest (hp|def|spd; default hp).
    #[command(name = "survive-evs")]
    SurviveEvs {
        attacker: String,
        defender: String,
        move_: String,
        /// Defensive stat to invest: hp | def | spd.
        #[arg(long, default_value = "hp")]
        stat: String,
        /// Tolerate up to N of the 16 rolls KO'ing you — survive the other
        /// 16−N. `--allow 1` = survive 15/16 (all but the highest roll), the
        /// common "live the roll" bogey. Default 0 = survive every roll.
        #[arg(long)]
        allow: Option<u8>,
        /// Alternative to --allow: target survival as a percent of rolls
        /// (e.g. `--chance 90`). Rounded up to a whole roll count.
        #[arg(long, conflicts_with = "allow")]
        chance: Option<u8>,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        terrain: Option<String>,
        #[arg(long)]
        spread: bool,
    },

    /// Min EVs for the attacker to guarantee the KO:
    /// `vgc ko-evs <atk> <def> <move>`. `--stat` picks the offensive stat
    /// (atk|spa; default atk).
    #[command(name = "ko-evs")]
    KoEvs {
        attacker: String,
        defender: String,
        move_: String,
        /// Offensive stat to invest: atk | spa.
        #[arg(long, default_value = "atk")]
        stat: String,
        /// Tolerate up to N of the 16 rolls failing to KO — KO on the other
        /// 16−N. `--allow 1` = KO on 15/16 (all but the lowest roll). Default
        /// 0 = guaranteed KO (every roll).
        #[arg(long)]
        allow: Option<u8>,
        /// Alternative to --allow: target KO chance as a percent of rolls
        /// (e.g. `--chance 90`). Rounded up to a whole roll count.
        #[arg(long, conflicts_with = "allow")]
        chance: Option<u8>,
        #[arg(long)]
        weather: Option<String>,
        #[arg(long)]
        terrain: Option<String>,
        #[arg(long)]
        spread: bool,
    },
}

fn parse_weather(s: &str) -> Result<Weather, String> {
    match s.to_ascii_lowercase().as_str() {
        "sun" | "harshsunshine" => Ok(Weather::Sun),
        "rain" => Ok(Weather::Rain),
        "sand" | "sandstorm" => Ok(Weather::Sand),
        "snow" | "hail" => Ok(Weather::Snow),
        other => Err(format!("unknown weather '{other}' (want sun|rain|sand|snow)")),
    }
}

fn parse_terrain(s: &str) -> Result<Terrain, String> {
    match s.to_ascii_lowercase().as_str() {
        "electric" => Ok(Terrain::Electric),
        "grassy" => Ok(Terrain::Grassy),
        "psychic" => Ok(Terrain::Psychic),
        "misty" => Ok(Terrain::Misty),
        other => Err(format!(
            "unknown terrain '{other}' (want electric|grassy|psychic|misty)"
        )),
    }
}

/// Pretty title-case of a dex slug for display: "landorustherian" isn't
/// reversible to "Landorus-Therian", so use the dex's canonical name.
fn display_species(slug: &str) -> String {
    vgc_engine_core::data::species_by_slug(slug)
        .map(|s| s.name.to_string())
        .unwrap_or_else(|| slug.to_string())
}

fn display_move(slug: &str) -> String {
    vgc_engine_core::data::move_by_slug(slug)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| slug.to_string())
}

/// Render the calc result to the human-readable block described in the
/// design:
/// ```text
/// Garchomp Earthquake vs. Landorus-Therian
///   132–156 (72.5–85.7%) — possible 2HKO
///   rolls: ...
/// ```
fn format_result(
    atk_name: &str,
    def_name: &str,
    move_name: &str,
    r: &DamageResult,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{atk_name} {move_name} vs. {def_name}\n"));

    if r.max == 0 {
        out.push_str("  0 (immune / no damage)\n");
        return out;
    }

    // KO label: prefer the exact single-hit verdict when the move KOs in
    // one (guaranteed / chance-to-OHKO); otherwise fall back to the
    // multi-hit NHKO label (2HKO/3HKO/…) from the roll convolution.
    let ko_tag = match &r.ko_chance {
        KoChance::Guaranteed => " — guaranteed OHKO".to_string(),
        KoChance::Chance { pct } => format!(" — {pct}% to OHKO"),
        KoChance::None => format!(" — {}", r.multi_hit.label()),
    };

    out.push_str(&format!(
        "  {}–{} ({:.1}–{:.1}%){}\n",
        r.min, r.max, r.min_pct, r.max_pct, ko_tag
    ));
    let rolls: Vec<String> = r.rolls.iter().map(|v| v.to_string()).collect();
    out.push_str(&format!("  rolls: {}\n", rolls.join(", ")));
    out
}

fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse();
    match cli.command {
        Command::Calc {
            attacker,
            defender,
            move_,
            weather,
            terrain,
            spread,
            crit,
            json,
        } => {
            let atk = QuickMon::parse(&attacker).map_err(|e| e.to_string())?;
            let def = QuickMon::parse(&defender).map_err(|e| e.to_string())?;

            let mut field = Field::none();
            if let Some(w) = weather {
                field.weather = parse_weather(&w)?;
            }
            if let Some(t) = terrain {
                field.terrain = parse_terrain(&t)?;
            }
            field.spread = spread;

            let result = calc(&atk, &def, &move_, field).map_err(|e| e.to_string())?;

            // `--crit` shows the crit row instead of the base row.
            let shown = if crit {
                result.crit.as_deref().unwrap_or(&result)
            } else {
                &result
            };

            if json {
                let s = serde_json::to_string_pretty(shown)
                    .map_err(|e| format!("json serialize: {e}"))?;
                println!("{s}");
                return Ok(ExitCode::SUCCESS);
            }

            let atk_name = display_species(&atk.species);
            let def_name = display_species(&def.species);
            // Resolve the move slug for display via the same resolver the
            // calc used (re-resolve is cheap and keeps the display in sync).
            let move_name = vgc_engine_core::calc::resolve_move(&move_)
                .map(|slug| display_move(&slug))
                .unwrap_or_else(|_| move_.clone());

            print!("{}", format_result(&atk_name, &def_name, &move_name, shown));
            Ok(ExitCode::SUCCESS)
        }

        Command::Survives {
            attacker,
            defender,
            move_,
            weather,
            terrain,
            spread,
        } => {
            let atk = QuickMon::parse(&attacker).map_err(|e| e.to_string())?;
            let def = QuickMon::parse(&defender).map_err(|e| e.to_string())?;
            let mut field = Field::none();
            if let Some(w) = weather {
                field.weather = parse_weather(&w)?;
            }
            if let Some(t) = terrain {
                field.terrain = parse_terrain(&t)?;
            }
            field.spread = spread;

            let r = calc(&atk, &def, &move_, field).map_err(|e| e.to_string())?;
            let lives = matches!(r.ko_chance, KoChance::None);
            let def_name = display_species(&def.species);
            let atk_name = display_species(&atk.species);
            let move_name = vgc_engine_core::calc::resolve_move(&move_)
                .map(|slug| display_move(&slug))
                .unwrap_or_else(|_| move_.clone());
            let verdict = if lives { "survives" } else { "does NOT survive" };
            let ko = match &r.ko_chance {
                KoChance::Guaranteed => "guaranteed OHKO".to_string(),
                KoChance::Chance { pct } => format!("{pct}% to OHKO"),
                KoChance::None => r.multi_hit.label(),
            };
            println!("{def_name} {verdict} {atk_name} {move_name} — {ko}");
            // Exit 0 iff it survives every roll (scriptable).
            Ok(if lives { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }

        Command::Outspeeds {
            a,
            b,
            weather,
            tailwind_a,
            tailwind_b,
            trick_room,
        } => {
            let ma = QuickMon::parse(&a).map_err(|e| e.to_string())?;
            let mb = QuickMon::parse(&b).map_err(|e| e.to_string())?;
            let w = match weather {
                Some(ref s) => parse_weather(s)?,
                None => Weather::None,
            };
            // Tailwind is per-side, so compute each speed under its own
            // context and compare directly (rather than the shared-context
            // `outspeeds`, which assumes one tailwind for both).
            let sa = speed_tier(
                &ma,
                SpeedContext { weather: w, tailwind: tailwind_a, trick_room },
            )
            .map_err(|e| e.to_string())?;
            let sb = speed_tier(
                &mb,
                SpeedContext { weather: w, tailwind: tailwind_b, trick_room },
            )
            .map_err(|e| e.to_string())?;
            let winner = match sa.cmp(&sb) {
                std::cmp::Ordering::Equal => SpeedWinner::Tie,
                std::cmp::Ordering::Greater => {
                    if trick_room { SpeedWinner::B } else { SpeedWinner::A }
                }
                std::cmp::Ordering::Less => {
                    if trick_room { SpeedWinner::A } else { SpeedWinner::B }
                }
            };
            let a_name = display_species(&ma.species);
            let b_name = display_species(&mb.species);
            let tr = if trick_room { " (Trick Room)" } else { "" };
            match winner {
                SpeedWinner::A => {
                    println!("{a_name} ({sa}) moves before {b_name} ({sb}){tr}")
                }
                SpeedWinner::B => {
                    println!("{b_name} ({sb}) moves before {a_name} ({sa}){tr}")
                }
                SpeedWinner::Tie => {
                    println!("speed tie: {a_name} and {b_name} both {sa}{tr}")
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Speed { mon, weather, tailwind } => {
            let m = QuickMon::parse(&mon).map_err(|e| e.to_string())?;
            let w = match weather {
                Some(ref s) => parse_weather(s)?,
                None => Weather::None,
            };
            let spe = speed_tier(&m, SpeedContext { weather: w, tailwind, trick_room: false })
                .map_err(|e| e.to_string())?;
            println!("{} — {} Spe", display_species(&m.species), spe);
            Ok(ExitCode::SUCCESS)
        }

        Command::Best {
            attacker,
            defender,
            moves,
            weather,
            terrain,
            spread,
        } => {
            let atk = QuickMon::parse(&attacker).map_err(|e| e.to_string())?;
            let def = QuickMon::parse(&defender).map_err(|e| e.to_string())?;
            let field = build_field(weather, terrain, spread)?;
            let move_list = split_moves(&moves);
            let refs: Vec<&str> = move_list.iter().map(|s| s.as_str()).collect();
            let best = best_move(&atk, &def, &refs, field).map_err(|e| e.to_string())?;
            let atk_name = display_species(&atk.species);
            let def_name = display_species(&def.species);
            println!("best: {}", fmt_move_damage(&atk_name, &def_name, &best));
            Ok(ExitCode::SUCCESS)
        }

        Command::Matchup {
            a,
            a_moves,
            b,
            b_moves,
            weather,
            terrain,
            spread,
            trick_room,
        } => {
            let ma = QuickMon::parse(&a).map_err(|e| e.to_string())?;
            let mb = QuickMon::parse(&b).map_err(|e| e.to_string())?;
            let field = build_field(weather.clone(), terrain, spread)?;
            let ctx = SpeedContext {
                weather: match weather {
                    Some(ref s) => parse_weather(s)?,
                    None => Weather::None,
                },
                tailwind: false,
                trick_room,
            };
            let av: Vec<String> = split_moves(&a_moves);
            let bv: Vec<String> = split_moves(&b_moves);
            let ar: Vec<&str> = av.iter().map(|s| s.as_str()).collect();
            let br: Vec<&str> = bv.iter().map(|s| s.as_str()).collect();
            let m = matchup(&ma, &ar, &mb, &br, field, ctx).map_err(|e| e.to_string())?;

            let a_name = display_species(&ma.species);
            let b_name = display_species(&mb.species);
            let tr = if trick_room { " (Trick Room)" } else { "" };
            let speed_line = match m.speed_winner {
                SpeedWinner::A => format!("{a_name} ({}) outspeeds {b_name} ({})", m.a_speed, m.b_speed),
                SpeedWinner::B => format!("{b_name} ({}) outspeeds {a_name} ({})", m.b_speed, m.a_speed),
                SpeedWinner::Tie => format!("speed tie ({})", m.a_speed),
            };
            println!("{a_name} vs. {b_name}{tr}");
            println!("  speed: {speed_line}");
            println!("  {}", fmt_move_damage(&a_name, &b_name, &m.a_best));
            println!("  {}", fmt_move_damage(&b_name, &a_name, &m.b_best));
            Ok(ExitCode::SUCCESS)
        }

        Command::SurviveEvs {
            attacker,
            defender,
            move_,
            stat,
            allow,
            chance,
            weather,
            terrain,
            spread,
        } => {
            let atk = QuickMon::parse(&attacker).map_err(|e| e.to_string())?;
            let def = QuickMon::parse(&defender).map_err(|e| e.to_string())?;
            let field = build_field(weather, terrain, spread)?;
            let ds = parse_def_stat(&stat)?;
            let target = resolve_target_rolls(allow, chance);
            let res = min_evs_to_survive(&atk, &def, &move_, ds, field, target)
                .map_err(|e| e.to_string())?;
            let def_name = display_species(&def.species);
            let atk_name = display_species(&atk.species);
            let s = stat.to_ascii_uppercase();
            let move_name = vgc_engine_core::calc::resolve_move(&move_)
                .map(|slug| display_move(&slug))
                .unwrap_or_else(|_| move_.clone());
            match res.evs {
                Some(ev) if target >= 16 => {
                    println!("{def_name} needs {ev} {s} EVs to survive {atk_name} {move_name} (every roll)");
                    Ok(ExitCode::SUCCESS)
                }
                Some(ev) => {
                    println!("{def_name} needs {ev} {s} EVs to survive {atk_name} {move_name} on {target}/16 rolls");
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    // Residual: report the truer picture instead of a flat
                    // "can't survive" — even maxed, how many rolls does it live?
                    let ko = 16 - res.rolls_at_max;
                    println!(
                        "{def_name} can't survive {atk_name} {move_name} on {target}/16 rolls — even at 252 {s} EVs it lives {}/16 ({}%); {ko} roll(s) KO",
                        res.rolls_at_max, res.pct_at_max
                    );
                    Ok(ExitCode::FAILURE)
                }
            }
        }

        Command::KoEvs {
            attacker,
            defender,
            move_,
            stat,
            allow,
            chance,
            weather,
            terrain,
            spread,
        } => {
            let atk = QuickMon::parse(&attacker).map_err(|e| e.to_string())?;
            let def = QuickMon::parse(&defender).map_err(|e| e.to_string())?;
            let field = build_field(weather, terrain, spread)?;
            let as_ = parse_atk_stat(&stat)?;
            let target = resolve_target_rolls(allow, chance);
            let res =
                min_evs_to_ko(&atk, &def, &move_, as_, field, target).map_err(|e| e.to_string())?;
            let def_name = display_species(&def.species);
            let atk_name = display_species(&atk.species);
            let s = stat.to_ascii_uppercase();
            let move_name = vgc_engine_core::calc::resolve_move(&move_)
                .map(|slug| display_move(&slug))
                .unwrap_or_else(|_| move_.clone());
            match res.evs {
                Some(ev) if target >= 16 => {
                    println!("{atk_name} needs {ev} {s} EVs to guarantee the KO on {def_name} with {move_name}");
                    Ok(ExitCode::SUCCESS)
                }
                Some(ev) => {
                    println!("{atk_name} needs {ev} {s} EVs to KO {def_name} with {move_name} on {target}/16 rolls");
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    println!(
                        "{atk_name} can't KO {def_name} with {move_name} on {target}/16 rolls — even at 252 {s} EVs it KOs on {}/16 ({}%)",
                        res.rolls_at_max, res.pct_at_max
                    );
                    Ok(ExitCode::FAILURE)
                }
            }
        }
    }
}

/// Resolve the `--allow N` / `--chance PCT` flags to a target roll count
/// (0..=16) for the EV-threshold search. `--allow N` tolerates N rolls
/// failing → target `16−N`. `--chance PCT` → `ceil(PCT/100 · 16)`. Neither →
/// 16 (strict: every roll). clap enforces the two are mutually exclusive.
fn resolve_target_rolls(allow: Option<u8>, chance: Option<u8>) -> u8 {
    if let Some(n) = allow {
        16u8.saturating_sub(n.min(16))
    } else if let Some(pct) = chance {
        (((pct.min(100) as u32) * 16 + 99) / 100) as u8
    } else {
        16
    }
}

/// Parse a defensive-stat flag (hp | def | spd).
fn parse_def_stat(s: &str) -> Result<DefStat, String> {
    match s.to_ascii_lowercase().as_str() {
        "hp" => Ok(DefStat::Hp),
        "def" => Ok(DefStat::Def),
        "spd" | "spdef" => Ok(DefStat::Spd),
        other => Err(format!("unknown defensive stat '{other}' (want hp|def|spd)")),
    }
}

/// Parse an offensive-stat flag (atk | spa).
fn parse_atk_stat(s: &str) -> Result<AtkStat, String> {
    match s.to_ascii_lowercase().as_str() {
        "atk" | "attack" => Ok(AtkStat::Atk),
        "spa" | "spatk" => Ok(AtkStat::Spa),
        other => Err(format!("unknown offensive stat '{other}' (want atk|spa)")),
    }
}

/// Build a `Field` from the shared weather/terrain/spread CLI flags.
fn build_field(
    weather: Option<String>,
    terrain: Option<String>,
    spread: bool,
) -> Result<Field, String> {
    let mut field = Field::none();
    if let Some(w) = weather {
        field.weather = parse_weather(&w)?;
    }
    if let Some(t) = terrain {
        field.terrain = parse_terrain(&t)?;
    }
    field.spread = spread;
    Ok(field)
}

/// Split a comma-separated move list, trimming and dropping empties.
fn split_moves(s: &str) -> Vec<String> {
    s.split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

/// One-line summary of a `MoveDamage`: "Chomp Dragon Claw: 90–107
/// (…%) — guaranteed 2HKO".
fn fmt_move_damage(atk_name: &str, def_name: &str, md: &MoveDamage) -> String {
    let r = &md.result;
    let move_name = display_move(&md.move_slug);
    let ko = match &r.ko_chance {
        KoChance::Guaranteed => "guaranteed OHKO".to_string(),
        KoChance::Chance { pct } => format!("{pct}% to OHKO"),
        KoChance::None => r.multi_hit.label(),
    };
    if r.max == 0 {
        return format!("{atk_name} {move_name} vs. {def_name}: 0 (immune)");
    }
    format!(
        "{atk_name} {move_name} vs. {def_name}: {}–{} ({:.1}–{:.1}%) — {}",
        r.min, r.max, r.min_pct, r.max_pct, ko
    )
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
