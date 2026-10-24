//! Output format and process exit codes.
//!
//! Every command has to answer two questions the same way: *what does stdout
//! look like* and *what does the shell see*.  Both used to be decided ad hoc —
//! `--json` existed on some commands only, and every failure was exit 1.
//!
//! Exit codes follow the usual convention so a script can tell "the command ran
//! and something is wrong with the store" apart from "you typed the command
//! wrong":
//!
//! | Code | Name | Meaning |
//! |------|------|---------|
//! | 0 | `Ok` | success |
//! | 1 | `Failure` | the operation ran and failed |
//! | 2 | `Usage` | bad arguments / unknown command (clap) |
//! | 3 | `Partial` | the operation ran but some items failed |

/// How a command should render its result on stdout.
///
/// The command modules use this as they move out of `main.rs`; until then the
/// only consumer is the test module below, hence the `dead_code` allow.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable tables and colours (the default).
    #[default]
    Table,
    /// Machine-readable JSON. Nothing else may be written to stdout.
    Json,
    /// Comma-separated values, one header row.
    Csv,
}

#[allow(dead_code)]
impl OutputFormat {
    /// Pick the format from the `--json` / `--csv` flags.
    ///
    /// `--json` wins when both are given; clap rejects that combination at the
    /// definition site for the commands that expose both.
    pub fn from_flags(json: bool, csv: bool) -> Self {
        match (json, csv) {
            (true, _) => OutputFormat::Json,
            (false, true) => OutputFormat::Csv,
            (false, false) => OutputFormat::Table,
        }
    }

    /// `true` when the output is machine-readable, i.e. when stdout must stay clean.
    pub fn is_machine(&self) -> bool {
        matches!(self, OutputFormat::Json | OutputFormat::Csv)
    }

    /// Stable identifier used in logs and tests.
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Table => "table",
            OutputFormat::Json => "json",
            OutputFormat::Csv => "csv",
        }
    }
}

/// Process exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Everything worked.
    Ok = 0,
    /// The operation ran and failed.
    Failure = 1,
    /// The command line was wrong (clap's own failures).
    Usage = 2,
    /// The operation ran, but some items failed (see the reported errors).
    Partial = 3,
}

impl ExitCode {
    /// The numeric code passed to `std::process::exit`.
    pub fn code(self) -> i32 {
        self as i32
    }
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> i32 {
        code.code()
    }
}

/// The help text appended to `modeld --help`, documenting the exit codes.
pub const EXIT_CODE_HELP: &str = "\
Exit codes:
  0  success
  1  the operation ran and failed
  2  invalid command line
  3  the operation ran but some items failed (e.g. a scan with per-file errors)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_pick_the_format() {
        assert_eq!(OutputFormat::from_flags(false, false), OutputFormat::Table);
        assert_eq!(OutputFormat::from_flags(true, false), OutputFormat::Json);
        assert_eq!(OutputFormat::from_flags(false, true), OutputFormat::Csv);
        // `--json` wins when both are present.
        assert_eq!(OutputFormat::from_flags(true, true), OutputFormat::Json);
    }

    #[test]
    fn machine_formats_are_flagged() {
        assert!(!OutputFormat::Table.is_machine());
        assert!(OutputFormat::Json.is_machine());
        assert!(OutputFormat::Csv.is_machine());
    }

    #[test]
    fn exit_codes_match_the_documented_numbers() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Failure.code(), 1);
        assert_eq!(ExitCode::Usage.code(), 2);
        assert_eq!(ExitCode::Partial.code(), 3);
        assert!(EXIT_CODE_HELP.contains("3  the operation ran but some items failed"));
    }
}
