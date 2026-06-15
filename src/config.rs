//! Configuration file parsing and validation.
//!
//! The config is a YAML file with three top-level sections:
//!
//! * `storage` – optional account name (may be provided on the CLI / via env
//!   var instead)
//! * `files`       – list of folder paths + filename glob patterns to scan
//! * `validations` – column-level rules, mapped to file paths by a prefix
//! * `validations` – per-file-pattern column rules
//!
//! All regex patterns in `ValidationRule::Regex` are pre-compiled at load
//! time so that configuration mistakes surface immediately.

use std::path::Path;

use serde::Deserialize;

use crate::error::{AppError, Result};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Root configuration object.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Optional storage account name (can be overridden by CLI / env).
    pub storage: Option<StorageConfig>,

    /// Ordered list of folder paths + filename patterns to discover CSV files.
    pub files: Vec<FileConfig>,

    /// Validation rules, each mapped to a set of file paths.
    pub validations: Vec<FileValidation>,

}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Storage-related configuration.
#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    /// Azure Storage account name.
    pub account: Option<String>,
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

/// Describes a folder inside the container to scan for CSV files.
#[derive(Debug, Deserialize)]
pub struct FileConfig {
    /// Folder path inside the container (e.g. `raw/orders`).
    pub path: String,

    /// Glob pattern for filenames (default: `*.csv`).
    #[serde(default = "default_file_pattern")]
    pub file_pattern: String,
}

fn default_file_pattern() -> String {
    "*.csv".to_string()
}

// ---------------------------------------------------------------------------
// Validation rules
// ---------------------------------------------------------------------------

/// Maps a set of column rules to files whose path starts with `file_pattern`.
#[derive(Debug, Deserialize, Clone)]
pub struct FileValidation {
    /// Path prefix that a file must start with to apply these rules
    /// (e.g. `raw/orders` matches `raw/orders/2024-01-01.csv`).
    pub file_pattern: String,

    /// Per-column validation rules.
    pub columns: Vec<ColumnValidation>,
}

/// All rules that apply to a single CSV column.
#[derive(Debug, Deserialize, Clone)]
pub struct ColumnValidation {
    /// Exact column header name as it appears in the CSV file.
    pub name: String,

    /// Ordered list of rules to enforce on every cell in this column.
    pub rules: Vec<ValidationRule>,
}

/// A single validation rule that can be applied to a CSV column.
///
/// Rules are represented as a YAML tagged union (discriminated by the `type`
/// key).  Example:
///
/// ```yaml
/// rules:
///   - type: not_null
///   - type: no_duplicates
///   - type: regex
///     pattern: "^ORD-[0-9]{6}$"
///     message: "Order ID must be ORD-XXXXXX"
///   - type: allowed_values
///     values: ["pending", "shipped", "delivered"]
///   - type: numeric_range
///     min: 0.0
///     max: 99999.99
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValidationRule {
    /// Column value must not be empty or whitespace-only.
    NotNull,

    /// No two rows may share the same (non-empty) value in this column.
    NoDuplicates,

    /// Every non-empty value must match the supplied regular expression.
    Regex {
        /// POSIX-compatible regex pattern.
        pattern: String,
        /// Optional human-readable message shown in violation reports.
        #[serde(default)]
        message: Option<String>,
    },

    /// String length (in characters) must be at least `value`.
    MinLength { value: usize },

    /// String length (in characters) must be no more than `value`.
    MaxLength { value: usize },

    /// Non-empty values must be one of the listed strings (case-sensitive).
    AllowedValues { values: Vec<String> },

    /// Non-empty values must be valid floats within `[min, max]`.
    /// Omit `min` or `max` to leave that bound unbounded.
    NumericRange {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },

    /// Non-empty values must be parseable as a whole number (`i64`).
    Integer,
}

// ---------------------------------------------------------------------------
// Config loading + self-validation
// ---------------------------------------------------------------------------

impl Config {
    /// Load and validate a config file from disk.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Config` if the file cannot be read, if the YAML is
    /// malformed, or if any regex patterns fail to compile.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            AppError::Config(format!(
                "Cannot read config file '{}': {}",
                path.display(),
                e
            ))
        })?;

        let config: Config = serde_yaml::from_str(&content).map_err(|e| {
            AppError::Config(format!(
                "Failed to parse config file '{}': {}",
                path.display(),
                e
            ))
        })?;

        config.validate()?;
        Ok(config)
    }

    /// Perform semantic validation beyond what serde provides.
    fn validate(&self) -> Result<()> {
        if self.files.is_empty() {
            return Err(AppError::Config(
                "Config must define at least one entry under `files`".to_string(),
            ));
        }

        if self.validations.is_empty() {
            return Err(AppError::Config(
                "Config must define at least one entry under `validations`".to_string(),
            ));
        }

        // Pre-compile every regex to catch typos at startup.
        for fv in &self.validations {
            for col in &fv.columns {
                for rule in &col.rules {
                    if let ValidationRule::Regex { pattern, .. } = rule {
                        regex::Regex::new(pattern).map_err(|e| {
                            AppError::Config(format!(
                                "Invalid regex '{}' in column '{}' \
                                 (file_pattern: '{}'): {}",
                                pattern, col.name, fv.file_pattern, e
                            ))
                        })?;
                    }
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `file_path` is considered to "belong" to `pattern`.
///
/// Matching is prefix-based on path segments so that `raw/orders` matches
/// `raw/orders/2024-01-01.csv` but *not* `raw/orders-archive/file.csv`.
pub fn path_matches_prefix(file_path: &str, pattern: &str) -> bool {
    if file_path == pattern {
        return true;
    }
    let with_sep = if pattern.ends_with('/') {
        pattern.to_string()
    } else {
        format!("{}/", pattern)
    };
    file_path.starts_with(&with_sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_prefix_exact_match() {
        assert!(path_matches_prefix("raw/orders", "raw/orders"));
    }

    #[test]
    fn path_prefix_subfolder_match() {
        assert!(path_matches_prefix(
            "raw/orders/2024-01-01.csv",
            "raw/orders"
        ));
    }

    #[test]
    fn path_prefix_no_false_positive() {
        assert!(!path_matches_prefix(
            "raw/orders-archive/file.csv",
            "raw/orders"
        ));
    }

    #[test]
    fn path_prefix_trailing_slash_in_pattern() {
        assert!(path_matches_prefix(
            "raw/orders/file.csv",
            "raw/orders/"
        ));
    }
}
