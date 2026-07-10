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
    calc, speed_tier, DamageResult, Field, KoChance, QuickMon, SpeedContext, SpeedWinner,
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
    }
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
