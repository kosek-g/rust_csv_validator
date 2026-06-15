//! Application-wide error types.
//!
//! `AppError` is used for structured, typed errors throughout the library
//! modules.  `main.rs` wraps everything in `anyhow::Result` so that the
//! top-level binary can attach extra context cheaply.

use thiserror::Error;

/// Every error variant the application can produce.
#[derive(Debug, Error)]
pub enum AppError {
    /// An error caused by a bad or missing configuration file.
    #[error("Configuration error: {0}")]
    Config(String),

    /// An error returned by the Azure / object-store layer.
    #[error("Storage error: {0}")]
    Storage(#[from] object_store::Error),

    /// An error while parsing a CSV file.
    #[error("CSV parsing error: {0}")]
    Csv(#[from] csv::Error),

    /// An error in a validation rule definition (e.g. invalid regex).
    #[error("Validation rule error: {0}")]
    Rule(String),

    /// A standard I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An error while parsing the YAML config file.
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// An invalid regular expression in the config.
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
}

/// Convenience alias used throughout the application.
pub type Result<T> = std::result::Result<T, AppError>;
