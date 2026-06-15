//! Core validation orchestrator.
//!
//! `Validator` receives raw CSV bytes and a reference to the loaded `Config`,
//! finds the matching `FileValidation` block for the given path, and runs
//! every column rule against the data.
//!
//! Column values are fully loaded into memory before rules execute so that
//! `NoDuplicates` can see the entire column at once.

use std::collections::HashMap;

use crate::config::{path_matches_prefix, Config, FileValidation};
use crate::error::{AppError, Result};
use crate::rules::{RuleEngine, ValidationViolation};

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// Summary of validation results for a single file.
#[derive(Debug)]
pub struct FileValidationResult {
    /// Full path of the validated file (relative to the storage root).
    pub file_path: String,
    /// All violations found in the file (empty ⇒ valid).
    pub violations: Vec<ValidationViolation>,
    /// Total number of data rows examined.
    pub rows_checked: usize,
    /// Number of columns that had at least one rule applied.
    pub columns_checked: usize,
}

impl FileValidationResult {
    /// `true` when no violations were found.
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Validator
// ---------------------------------------------------------------------------

/// Stateful validator that wraps the rule engine and config.
pub struct Validator<'cfg> {
    config: &'cfg Config,
    engine: RuleEngine,
}

impl<'cfg> Validator<'cfg> {
    /// Create a new validator bound to the given config.
    pub fn new(config: &'cfg Config) -> Self {
        Self {
            config,
            engine: RuleEngine::new(),
        }
    }

    /// Validate `content` (raw CSV bytes) as the file at `file_path`.
    ///
    /// If no `FileValidation` block in the config matches `file_path` the
    /// file is considered unchecked and an empty-violation result is returned
    /// (a debug-level log message is emitted in this case).
    pub fn validate_file(
        &mut self,
        file_path: &str,
        content: &[u8],
    ) -> Result<FileValidationResult> {
        // Clone the matching validation block to satisfy the borrow checker:
        // we need `&mut self.engine` while iterating, and cloning avoids an
        // aliasing conflict with `self.config`.
        let validation = self
            .config
            .validations
            .iter()
            .find(|fv| path_matches_prefix(file_path, &fv.file_pattern))
            .cloned();

        match validation {
            None => {
                tracing::debug!(
                    file = file_path,
                    "No validation rules matched — file skipped"
                );
                Ok(FileValidationResult {
                    file_path: file_path.to_string(),
                    violations: vec![],
                    rows_checked: 0,
                    columns_checked: 0,
                })
            }
            Some(fv) => self.run_validation(file_path, content, &fv),
        }
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn run_validation(
        &mut self,
        file_path: &str,
        content: &[u8],
        validation: &FileValidation,
    ) -> Result<FileValidationResult> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .trim(csv::Trim::All)
            .from_reader(content);

        let headers = reader.headers()?.clone();

        // Resolve column indices up-front so we can report missing columns
        // before processing any rows.
        let column_indices = resolve_column_indices(&headers, validation, file_path)?;

        // Collect all column values in a single pass through the CSV data.
        // Key: column name, Value: Vec<(row_number, owned_value)>
        let mut column_data: HashMap<&str, Vec<(usize, String)>> = validation
            .columns
            .iter()
            .map(|c| (c.name.as_str(), Vec::new()))
            .collect();

        let mut rows_checked: usize = 0;
        for (record_index, record_result) in reader.records().enumerate() {
            let record = record_result?;
            let row_number = record_index + 2; // row 1 is the header
            rows_checked += 1;

            for col_validation in &validation.columns {
                let idx = column_indices[col_validation.name.as_str()];
                let value = record.get(idx).unwrap_or("").to_string();
                column_data
                    .entry(col_validation.name.as_str())
                    .or_default()
                    .push((row_number, value));
            }
        }

        // Apply rules to each column.
        let mut all_violations = Vec::new();
        for col_validation in &validation.columns {
            let owned_values = column_data
                .get(col_validation.name.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);

            // Convert owned values to (row, &str) pairs for the engine.
            let borrowed: Vec<(usize, &str)> = owned_values
                .iter()
                .map(|(row, v)| (*row, v.as_str()))
                .collect();

            for rule in &col_validation.rules {
                let violations = self.engine.apply(rule, &col_validation.name, &borrowed);
                all_violations.extend(violations);
            }
        }

        Ok(FileValidationResult {
            file_path: file_path.to_string(),
            violations: all_violations,
            rows_checked,
            columns_checked: validation.columns.len(),
        })
    }
}

/// Build a map of `column_name → CSV column index`.
///
/// Returns `AppError::Rule` for any column that cannot be found in the
/// CSV headers, listing all available headers in the message.
fn resolve_column_indices<'a>(
    headers: &csv::StringRecord,
    validation: &'a FileValidation,
    file_path: &str,
) -> Result<HashMap<&'a str, usize>> {
    let mut indices = HashMap::new();
    for col in &validation.columns {
        match headers.iter().position(|h| h == col.name) {
            Some(idx) => {
                indices.insert(col.name.as_str(), idx);
            }
            None => {
                let available: Vec<&str> = headers.iter().collect();
                return Err(AppError::Rule(format!(
                    "Column '{}' not found in '{}'. Available columns: [{}]",
                    col.name,
                    file_path,
                    available.join(", ")
                )));
            }
        }
    }
    Ok(indices)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ColumnValidation, FileConfig, FileValidation, ValidationRule};

    fn make_config(file_pattern: &str, columns: Vec<ColumnValidation>) -> Config {
        Config {
            storage: None,
            files: vec![FileConfig {
                path: file_pattern.to_string(),
                file_pattern: "*.csv".to_string(),
            }],
            validations: vec![FileValidation {
                file_pattern: file_pattern.to_string(),
                columns,
            }],
        }
    }

    #[test]
    fn valid_csv_produces_no_violations() {
        let config = make_config(
            "raw/orders",
            vec![ColumnValidation {
                name: "order_id".to_string(),
                rules: vec![ValidationRule::NotNull],
            }],
        );

        let csv = b"order_id,amount\nORD-000001,10.00\nORD-000002,20.00\n";
        let mut validator = Validator::new(&config);
        let result = validator
            .validate_file("raw/orders/test.csv", csv)
            .unwrap();

        assert!(result.is_valid());
        assert_eq!(result.rows_checked, 2);
        assert_eq!(result.columns_checked, 1);
    }

    #[test]
    fn null_in_required_column_is_caught() {
        let config = make_config(
            "raw/orders",
            vec![ColumnValidation {
                name: "order_id".to_string(),
                rules: vec![ValidationRule::NotNull],
            }],
        );

        let csv = b"order_id,amount\nORD-000001,10.00\n,20.00\n";
        let mut validator = Validator::new(&config);
        let result = validator
            .validate_file("raw/orders/test.csv", csv)
            .unwrap();

        assert!(!result.is_valid());
        assert_eq!(result.violations.len(), 1);
        assert_eq!(result.violations[0].rule, "not_null");
    }

    #[test]
    fn missing_column_returns_error() {
        let config = make_config(
            "raw/orders",
            vec![ColumnValidation {
                name: "nonexistent_column".to_string(),
                rules: vec![ValidationRule::NotNull],
            }],
        );

        let csv = b"order_id,amount\nORD-000001,10.00\n";
        let mut validator = Validator::new(&config);
        let result = validator.validate_file("raw/orders/test.csv", csv);

        assert!(result.is_err());
    }

    #[test]
    fn unmatched_file_path_returns_empty_result() {
        let config = make_config(
            "raw/orders",
            vec![ColumnValidation {
                name: "order_id".to_string(),
                rules: vec![ValidationRule::NotNull],
            }],
        );

        let csv = b"some_col\nvalue\n";
        let mut validator = Validator::new(&config);
        // "raw/customers" does not match "raw/orders"
        let result = validator
            .validate_file("raw/customers/test.csv", csv)
            .unwrap();

        assert!(result.is_valid());
        assert_eq!(result.rows_checked, 0);
    }

    #[test]
    fn duplicate_in_id_column_is_caught() {
        let config = make_config(
            "raw/orders",
            vec![ColumnValidation {
                name: "order_id".to_string(),
                rules: vec![ValidationRule::NoDuplicates],
            }],
        );

        let csv =
            b"order_id,amount\nORD-000001,10.00\nORD-000002,20.00\nORD-000001,30.00\n";
        let mut validator = Validator::new(&config);
        let result = validator
            .validate_file("raw/orders/test.csv", csv)
            .unwrap();

        assert!(!result.is_valid());
        assert_eq!(result.violations[0].rule, "no_duplicates");
    }
}
