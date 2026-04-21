//! CLI argument parsing for the macrocosmo game binary.
//!
//! Hand-rolled (no external crates) parser for the small set of flags
//! needed by #214 observer mode and related features:
//!
//! ```text
//! macrocosmo [--no-player] [--seed N] [--time-horizon H] [--speed S] [--help]
//! ```
//!
//! The parser is intentionally minimal. Unknown flags cause an error
//! containing the help text so `main.rs` can print it and exit.

/// Parsed command-line arguments.
///
/// All fields default to `false`/`None` — i.e. normal (player) mode with
/// no seed / horizon / speed overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliArgs {
    /// When `true`, run in observer mode — no Player entity spawned, N
    /// full NPC Empire entities spawned instead.
    pub no_player: bool,
    /// When `true`, the player empire is also controlled by the AI policy.
    /// Allows observing AI behavior on the player empire in both headed
    /// and headless mode.
    pub ai_player: bool,
    /// When `true`, run in observer mode with a player empire that is
    /// AI-controlled. The UI shows all empires but commands are disabled.
    /// Implies `ai_player = true`.
    pub observer: bool,
    /// Optional deterministic seed for galaxy generation.
    pub seed: Option<u64>,
    /// Optional time horizon (hexadies). When reached in observer mode
    /// the app exits automatically.
    pub time_horizon: Option<i64>,
    /// Optional initial game speed (hexadies per real second).
    pub speed: Option<f64>,
}

/// Help text printed on `--help` or on parse errors.
pub const HELP_TEXT: &str = "\
Macrocosmo — space 4X strategy game

USAGE:
    macrocosmo [OPTIONS]

OPTIONS:
    --no-player             Run in observer mode (no Player entity,
                            NPC factions only). Intended for AI balance
                            verification and demos.
    --ai-player             Let the AI policy also control the player
                            empire. Useful for observing AI behavior
                            alongside normal gameplay or headless tests.
    --observer              God-view observer mode. The player empire is
                            AI-controlled and commands are disabled. All
                            empire activity is visible. Implies --ai-player.
    --seed <N>              Deterministic seed for galaxy generation.
                            Works with or without --no-player.
    --time-horizon <H>      Auto-exit observer mode once GameClock.elapsed
                            reaches H hexadies. Only active with --no-player.
    --speed <S>             Initial game speed (hexadies per real second).
                            Decimal values accepted.
    --help, -h              Print this help text and exit.
";

impl CliArgs {
    /// Parse arguments from `std::env::args()`. On error prints the error
    /// and help text to stderr, then returns `CliArgs::default()`. On
    /// `--help` prints help to stdout and exits the process.
    pub fn parse() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match Self::parse_from(args) {
            Ok(parsed) => parsed,
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        }
    }

    /// Parse from an arbitrary iterator of arguments (test-friendly).
    /// Returns `Err(String)` on unknown flags, missing values, or
    /// unparseable numbers. The error message contains the help text.
    ///
    /// `--help` / `-h` short-circuits and returns the help text as an
    /// error so the caller can print it to stdout. `parse()` handles
    /// this differently (exit 0 vs exit 2).
    pub fn parse_from<I>(iter: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut out = CliArgs::default();
        let mut iter = iter.into_iter();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    // Caller can distinguish by presence of error; we treat
                    // --help as a request rather than a true error.
                    return Err(HELP_TEXT.to_string());
                }
                "--no-player" => {
                    out.no_player = true;
                }
                "--ai-player" => {
                    out.ai_player = true;
                }
                "--observer" => {
                    out.observer = true;
                    out.ai_player = true;
                }
                "--seed" => {
                    let v = iter
                        .next()
                        .ok_or_else(|| format!("error: --seed requires a value\n\n{HELP_TEXT}"))?;
                    let n: u64 = v.parse().map_err(|_| {
                        format!("error: --seed value '{v}' is not a valid u64\n\n{HELP_TEXT}")
                    })?;
                    out.seed = Some(n);
                }
                "--time-horizon" => {
                    let v = iter.next().ok_or_else(|| {
                        format!("error: --time-horizon requires a value\n\n{HELP_TEXT}")
                    })?;
                    let n: i64 = v.parse().map_err(|_| {
                        format!(
                            "error: --time-horizon value '{v}' is not a valid i64\n\n{HELP_TEXT}"
                        )
                    })?;
                    out.time_horizon = Some(n);
                }
                "--speed" => {
                    let v = iter
                        .next()
                        .ok_or_else(|| format!("error: --speed requires a value\n\n{HELP_TEXT}"))?;
                    let n: f64 = v.parse().map_err(|_| {
                        format!("error: --speed value '{v}' is not a valid number\n\n{HELP_TEXT}")
                    })?;
                    out.speed = Some(n);
                }
                other => {
                    return Err(format!("error: unknown argument '{other}'\n\n{HELP_TEXT}"));
                }
            }
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CliArgs, String> {
        CliArgs::parse_from(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn cli_parses_defaults_when_empty() {
        let a = parse(&[]).expect("no args ok");
        assert_eq!(a, CliArgs::default());
        assert!(!a.no_player);
        assert!(a.seed.is_none());
        assert!(a.time_horizon.is_none());
        assert!(a.speed.is_none());
    }

    #[test]
    fn cli_parses_no_player_flag() {
        let a = parse(&["--no-player"]).expect("parse ok");
        assert!(a.no_player);
    }

    #[test]
    fn cli_parses_seed_and_horizon() {
        let a = parse(&["--no-player", "--seed", "42", "--time-horizon", "600"]).expect("parse ok");
        assert!(a.no_player);
        assert_eq!(a.seed, Some(42));
        assert_eq!(a.time_horizon, Some(600));
    }

    #[test]
    fn cli_parses_speed() {
        let a = parse(&["--speed", "4"]).expect("parse ok");
        assert_eq!(a.speed, Some(4.0));
        let a = parse(&["--speed", "0.5"]).expect("parse ok");
        assert_eq!(a.speed, Some(0.5));
    }

    #[test]
    fn cli_rejects_unknown_flag() {
        let err = parse(&["--not-a-flag"]).expect_err("should reject");
        assert!(err.contains("unknown argument"));
        assert!(err.contains("--not-a-flag"));
    }

    #[test]
    fn cli_rejects_seed_without_value() {
        let err = parse(&["--seed"]).expect_err("should reject");
        assert!(err.contains("--seed"));
        assert!(err.contains("requires a value"));
    }

    #[test]
    fn cli_rejects_seed_non_numeric() {
        let err = parse(&["--seed", "abc"]).expect_err("should reject");
        assert!(err.contains("not a valid u64"));
    }

    #[test]
    fn cli_rejects_horizon_non_numeric() {
        let err = parse(&["--time-horizon", "xyz"]).expect_err("should reject");
        assert!(err.contains("not a valid i64"));
    }

    #[test]
    fn cli_help_returns_help_text() {
        let err = parse(&["--help"]).expect_err("help surfaces via Err");
        assert!(err.contains("USAGE"));
        assert!(err.contains("--no-player"));
        let err = parse(&["-h"]).expect_err("help surfaces via Err");
        assert!(err.contains("USAGE"));
    }

    #[test]
    fn cli_parses_observer_flag() {
        let a = parse(&["--observer"]).expect("parse ok");
        assert!(a.observer);
        assert!(a.ai_player, "--observer implies --ai-player");
        assert!(!a.no_player);
    }

    #[test]
    fn cli_observer_with_speed_and_seed() {
        let a = parse(&["--observer", "--seed", "99", "--speed", "4"]).expect("parse ok");
        assert!(a.observer);
        assert!(a.ai_player);
        assert_eq!(a.seed, Some(99));
        assert_eq!(a.speed, Some(4.0));
    }

    #[test]
    fn cli_combination_all_flags() {
        let a = parse(&[
            "--no-player",
            "--seed",
            "123",
            "--time-horizon",
            "10000",
            "--speed",
            "8.0",
        ])
        .expect("parse ok");
        assert!(a.no_player);
        assert_eq!(a.seed, Some(123));
        assert_eq!(a.time_horizon, Some(10000));
        assert_eq!(a.speed, Some(8.0));
    }
}
