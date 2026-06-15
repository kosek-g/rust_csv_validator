//! Integration tests for the csv_validator pipeline.
//!
//! These tests exercise the `Config → Validator → FileValidationResult`
//! pipeline end-to-end using on-disk CSV fixtures.  No Azure connection is
//! required.

use std::path::PathBuf;

// Re-export internal modules for white-box testing.
use csv_validator::config::Config;
use csv_validator::validator::Validator;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn load_config() -> Config {
    Config::from_file(&fixtures_dir().join("test_config.yaml"))
        .expect("test_config.yaml must be valid")
}

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(name))
        .unwrap_or_else(|_| panic!("fixture '{}' must exist", name))
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[test]
fn valid_orders_file_produces_no_violations() {
    let config = load_config();
    let content = read_fixture("valid_orders.csv");

    let mut validator = Validator::new(&config);
    let result = validator
        .validate_file("raw/orders/valid_orders.csv", &content)
        .expect("validation must not error on well-formed CSV");

    assert!(
        result.is_valid(),
        "Expected no violations, got: {:#?}",
        result.violations
    );
    assert_eq!(result.rows_checked, 5);
    assert_eq!(result.columns_checked, 5); // order_id, email, amount, currency, status
}

// ---------------------------------------------------------------------------
// Failure paths
// ---------------------------------------------------------------------------

#[test]
fn invalid_orders_file_produces_expected_violations() {
    let config = load_config();
    let content = read_fixture("invalid_orders.csv");

    let mut validator = Validator::new(&config);
    let result = validator
        .validate_file("raw/orders/invalid_orders.csv", &content)
        .expect("validation must not error even on bad data");

    assert!(!result.is_valid(), "Expected violations but got none");

    // Collect rule names for assertion clarity.
    let rules: Vec<&str> = result.violations.iter().map(|v| v.rule.as_str()).collect();

    // Row 2: order_id is empty — not_null
    assert!(
        rules.contains(&"not_null"),
        "Expected a not_null violation"
    );

    // Row 3: email is invalid — regex
    assert!(
        rules.contains(&"regex"),
        "Expected a regex violation for email"
    );

    // Rows 1 and 4 share ORD-000001 — no_duplicates
    assert!(
        rules.contains(&"no_duplicates"),
        "Expected a no_duplicates violation"
    );

    // Row 4: amount = -5.00 — numeric_range
    assert!(
        rules.contains(&"numeric_range"),
        "Expected a numeric_range violation"
    );

    // Row 5: currency = CHF and status = unknown_status — allowed_values
    assert!(
        rules.contains(&"allowed_values"),
        "Expected an allowed_values violation"
    );
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_csv_with_valid_headers_produces_no_violations() {
    let config = load_config();
    // CSV with headers only and no data rows.
    let csv = b"order_id,customer_id,email,amount,currency,status\n";

    let mut validator = Validator::new(&config);
    let result = validator
        .validate_file("raw/orders/empty.csv", csv)
        .expect("empty file must not error");

    assert!(result.is_valid());
    assert_eq!(result.rows_checked, 0);
}

#[test]
fn file_outside_configured_paths_is_skipped() {
    let config = load_config();
    let csv = b"some_col\nvalue\n";

    let mut validator = Validator::new(&config);
    // This path does not match "raw/orders".
    let result = validator
        .validate_file("raw/invoices/file.csv", csv)
        .expect("unmatched path must not error");

    assert!(result.is_valid());
    assert_eq!(result.rows_checked, 0, "unmatched file should not be checked");
}

#[test]
fn missing_column_in_csv_returns_error() {
    let config = load_config();
    // CSV is missing required columns.
    let csv = b"unrelated_col\nsome_value\n";

    let mut validator = Validator::new(&config);
    let result = validator.validate_file("raw/orders/bad_schema.csv", csv);

    assert!(
        result.is_err(),
        "Missing column must return an error, not empty violations"
    );
}
