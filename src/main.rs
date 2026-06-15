//! Entry point for `csv_validator`.
//!
//! Supports two run modes:
//!
//! * **Local mode** (`--local-file PATH`) — validates a single file from the
//!   local filesystem.  No Azure credentials are required.
//! * **Azure mode** (`--container NAME`) — lists CSV files in an ADLS Gen2
//!   container and validates them all.
//!
//! In both modes the validation rules are loaded from the YAML config file
//! and the report is printed in the requested format.  The process exits with
//! code 0 (all valid) or 1 (failures found / error).

#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod cli;

use csv_validator::config;
use csv_validator::reporter;
use csv_validator::storage;
use csv_validator::validator;

use std::time::Instant;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

use cli::Cli;
use config::Config;
use reporter::ValidationReport;
use storage::StorageClient;
use validator::Validator;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ── Logging ──────────────────────────────────────────────────────────
    let default_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    fmt::Subscriber::builder()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(default_level)),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    // ── Config ───────────────────────────────────────────────────────────
    tracing::info!(path = %cli.config_location.display(), "Loading configuration");

    let config = Config::from_file(&cli.config_location).with_context(|| {
        format!(
            "Failed to load config from '{}'",
            cli.config_location.display()
        )
    })?;

    if cli.dry_run {
        println!(
            "Config '{}' is valid — dry-run complete.",
            cli.config_location.display()
        );
        return Ok(());
    }

    // ── Run mode: local file vs Azure ─────────────────────────────────────
    let start = Instant::now();
    let mut results = Vec::new();
    let mut validator = Validator::new(&config);
    let report_label: String;

    if let Some(local_path) = &cli.local_file {
        // ── Local file mode ───────────────────────────────────────────────
        tracing::info!(path = %local_path.display(), "Local file mode — skipping Azure");

        report_label = local_path
            .parent()
            .and_then(|p| p.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("local")
            .to_string();

        let content = std::fs::read(local_path)
            .with_context(|| format!("Failed to read '{}'", local_path.display()))?;

        // Use just the filename as the key so config `file_pattern` can
        // match on `"logs.csv"` rather than the full absolute path.
        let file_key = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file.csv");

        tracing::info!(file = file_key, "Validating");

        let result = validator
            .validate_file(file_key, &content)
            .with_context(|| format!("Validation error in '{}'", local_path.display()))?;

        results.push(result);
    } else {
        // ── Azure mode ────────────────────────────────────────────────────
        let container = cli.container.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Either --container or --local-file must be provided.\n\
                 Run `csv_validator --help` for usage."
            )
        })?;

        report_label = container.to_string();

        // Account name: CLI flag > env var > config file.
        let account = cli
            .storage_account
            .clone()
            .or_else(|| config.storage.as_ref().and_then(|s| s.account.clone()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Azure Storage account name is required in Azure mode.\n\
                     Supply it via --account, AZURE_STORAGE_ACCOUNT env var, \
                     or storage.account in the config file."
                )
            })?;

        tracing::info!(account = %account, container = %container, "Connecting to Azure Storage");

        let client = StorageClient::new(&account, container)
            .context("Failed to initialise Azure Storage client")?;

        'outer: for file_config in &config.files {
            tracing::info!(
                path = %file_config.path,
                pattern = %file_config.file_pattern,
                "Discovering files"
            );

            let files = client
                .list_files(&file_config.path, &file_config.file_pattern)
                .await
                .with_context(|| {
                    format!(
                        "Failed to list files at '{}/{}'",
                        container, file_config.path
                    )
                })?;

            if files.is_empty() {
                tracing::warn!(path = %file_config.path, "No CSV files found — check the path and pattern");
            } else {
                tracing::info!(count = files.len(), path = %file_config.path, "Found files");
            }

            for file_path in &files {
                tracing::info!(file = %file_path, "Validating");

                let content = client
                    .read_file(file_path)
                    .await
                    .with_context(|| format!("Failed to read '{file_path}'"))?;

                let result = validator
                    .validate_file(file_path, &content)
                    .with_context(|| format!("Validation error in '{file_path}'"))?;

                if cli.fail_fast && !result.is_valid() {
                    tracing::debug!(file = %file_path, "fail-fast triggered — stopping early");
                    results.push(result);
                    break 'outer;
                }

                results.push(result);
            }
        }
    }

    // ── Build report ─────────────────────────────────────────────────────
    let report = ValidationReport {
        results,
        container: report_label,
        duration: start.elapsed(),
    };

    // ── Print report ─────────────────────────────────────────────────────
    match cli.output.as_str() {
        "json" => report.print_json(),
        "summary" => report.print_summary(),
        _ => report.print_pretty(),
    }

    // ── Exit code ─────────────────────────────────────────────────────────
    if !report.is_valid() {
        std::process::exit(1);
    }

    Ok(())
}
