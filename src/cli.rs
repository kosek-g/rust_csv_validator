//! CLI argument definitions using `clap` derive macros.
//!
//! All flags, their short forms, environment-variable overrides, and
//! detailed help strings are declared here to keep `main.rs` clean.

use clap::{ArgAction, Parser};
use std::path::PathBuf;

/// CSV Data Quality Validator
///
/// Reads CSV files from an Azure Data Lake Storage (ADLS Gen2) container and
/// validates them against expectations defined in a YAML configuration file.
/// Alternatively, pass `--local-file` to validate a single local CSV file
/// without any cloud connection.
///
/// Authentication is performed via a Storage Account Key which must be
/// supplied through the `AZURE_STORAGE_KEY` environment variable.  The account
/// name can be given on the command line, in the config file, or via the
/// `AZURE_STORAGE_ACCOUNT` environment variable.
///
/// Examples:
///
///   # Validate a local file (no Azure credentials required)
///   `csv_validator` --local-file /data/logs.csv --config config/logs.yaml
///
///   # Azure mode — scan a container
///   `csv_validator` --container raw-data --config config/rules.yaml
///
///   # JSON output (useful in CI)
///   `csv_validator` -c raw-data -C config/rules.yaml --output json
///
///   # Validate config syntax only
///   `csv_validator` -C config/rules.yaml --dry-run
#[derive(Debug, Parser)]
#[command(
    name = "csv_validator",
    version,
    author,
    about = "Data quality CLI framework for CSV files in Azure Data Lake Storage",
    long_about = None,
    help_template = "\
{before-help}{name} {version}
{author-with-newline}
{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}",
)]
pub struct Cli {
    /// Azure Storage container (filesystem) name.
    ///
    /// The container that holds the CSV files you want to validate.  This
    /// maps to an ADLS Gen2 filesystem or a Blob Storage container.
    ///
    /// Not required when `--local-file` is used.
    #[arg(
        short = 'c',
        long = "container",
        env = "CSV_VALIDATOR_CONTAINER",
        value_name = "CONTAINER",
        help = "Azure Storage container name (not needed with --local-file)"
    )]
    pub container: Option<String>,

    /// Validate a single local CSV file instead of connecting to Azure.
    ///
    /// Reads the file from the local filesystem.  The `--container` and
    /// `--account` flags are ignored in this mode.  Only the `validations`
    /// section of the config is used — the `files` section is skipped.
    #[arg(
        short = 'l',
        long = "local-file",
        value_name = "PATH",
        help = "Path to a local CSV file to validate (skips Azure)"
    )]
    pub local_file: Option<PathBuf>,

    /// Path to the YAML validation config file.
    ///
    /// The config file specifies which paths to scan inside the container,
    /// the validation rules for each column, and the alert settings.
    /// See `config/example_config.yaml` for a fully annotated template.
    #[arg(
        short = 'C',
        long = "config",
        env = "CSV_VALIDATOR_CONFIG",
        value_name = "FILE",
        help = "Path to the YAML validation configuration file"
    )]
    pub config_location: PathBuf,

    /// Azure Storage account name.
    ///
    /// Can also be provided via the `AZURE_STORAGE_ACCOUNT` environment
    /// variable or the `storage.account` key in the config file.
    /// Command-line flag > environment variable > config file.
    #[arg(
        long = "account",
        env = "AZURE_STORAGE_ACCOUNT",
        value_name = "ACCOUNT",
        help = "Azure Storage account name [env: AZURE_STORAGE_ACCOUNT]"
    )]
    pub storage_account: Option<String>,

    /// Output format for the validation report.
    ///
    /// - `pretty`   – human-readable coloured table (default, best for interactive use)
    /// - `json`     – machine-readable JSON (best for CI/CD pipelines)
    /// - `summary`  – single-line pass/fail summary (best for scripts)
    #[arg(
        short = 'o',
        long = "output",
        default_value = "pretty",
        value_parser = ["pretty", "json", "summary"],
        value_name = "FORMAT",
        help = "Output format: pretty | json | summary  [default: pretty]"
    )]
    pub output: String,

    /// Stop processing on the first validation failure.
    ///
    /// By default the validator checks every file and collects all
    /// violations.  Use this flag to abort immediately on the first failed
    /// file (useful when checking many large files).
    #[arg(
        long = "fail-fast",
        action = ArgAction::SetTrue,
        help = "Stop on first validation failure"
    )]
    pub fail_fast: bool,

    /// Check the config file for syntax errors without connecting to Azure.
    ///
    /// All YAML parsing and regex pre-compilation is performed.  If the
    /// config is valid the exit code is 0; otherwise it is 1 with a
    /// descriptive error message.
    #[arg(
        long = "dry-run",
        action = ArgAction::SetTrue,
        help = "Validate config syntax only — no network calls"
    )]
    pub dry_run: bool,

    /// Increase log verbosity.
    ///
    /// - `-v`    INFO-level messages (connection events, file counts)
    /// - `-vv`   DEBUG-level messages (rule details, row-level progress)
    /// - `-vvv`  TRACE-level messages (all internal state)
    ///
    /// Logs are always written to **stderr** so they do not pollute stdout.
    #[arg(
        short = 'v',
        long = "verbose",
        action = ArgAction::Count,
        help = "Increase log verbosity (-v info, -vv debug, -vvv trace)"
    )]
    pub verbose: u8,
}
