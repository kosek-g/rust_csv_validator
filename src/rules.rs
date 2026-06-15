//! Validation rule engine.
//!
//! `RuleEngine` is the stateful core that applies individual `ValidationRule`
//! variants to column data.  It caches compiled regexes so that patterns
//! shared across many files are only compiled once.
//!
//! The engine receives pre-collected column data as a slice of
//! `(row_number, value)` pairs where `row_number` is 1-based and accounts
//! for the header row (data starts at row 2).

use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::config::ValidationRule;
use crate::error::Result;

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// A single constraint violation found during validation.
#[derive(Debug, Clone)]
pub struct ValidationViolation {
    /// Short rule identifier (e.g. `"not_null"`, `"regex"`).
    pub rule: String,
    /// Name of the column that produced the violation.
    pub column: String,
    /// 1-based row number in the CSV file (header = row 1, first data = row 2).
    pub row: usize,
    /// The offending cell value (may be empty for `not_null` violations).
    pub value: String,
    /// Human-readable description of the violation.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Rule engine
// ---------------------------------------------------------------------------

/// Stateful engine that applies rules and caches compiled regexes.
#[derive(Default)]
pub struct RuleEngine {
    regex_cache: HashMap<String, Regex>,
}

impl RuleEngine {
    /// Create a new, empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a single `rule` to all `values` in a column.
    ///
    /// Returns a (possibly empty) list of violations.
    pub fn apply(
        &mut self,
        rule: &ValidationRule,
        column: &str,
        values: &[(usize, &str)],
    ) -> Vec<ValidationViolation> {
        match rule {
            ValidationRule::NotNull => self.check_not_null(column, values),
            ValidationRule::NoDuplicates => self.check_no_duplicates(column, values),
            ValidationRule::Regex { pattern, message } => self
                .check_regex(column, values, pattern, message.as_deref())
                .unwrap_or_else(|e| {
                    vec![ValidationViolation {
                        rule: "regex".to_string(),
                        column: column.to_string(),
                        row: 0,
                        value: String::new(),
                        message: format!("Regex compilation error: {}", e),
                    }]
                }),
            ValidationRule::MinLength { value: min } => {
                self.check_min_length(column, values, *min)
            }
            ValidationRule::MaxLength { value: max } => {
                self.check_max_length(column, values, *max)
            }
            ValidationRule::AllowedValues { values: allowed } => {
                self.check_allowed_values(column, values, allowed)
            }
            ValidationRule::NumericRange { min, max } => {
                self.check_numeric_range(column, values, *min, *max)
            }
            ValidationRule::Integer => self.check_integer(column, values),
        }
    }

    // ------------------------------------------------------------------
    // Rule implementations
    // ------------------------------------------------------------------

    fn check_not_null(
        &self,
        column: &str,
        values: &[(usize, &str)],
    ) -> Vec<ValidationViolation> {
        values
            .iter()
            .filter(|(_, v)| v.trim().is_empty())
            .map(|(row, _)| ValidationViolation {
                rule: "not_null".to_string(),
                column: column.to_string(),
                row: *row,
                value: String::new(),
                message: format!(
                    "Column '{}' contains a NULL/empty value at row {}",
                    column, row
                ),
            })
            .collect()
    }

    fn check_no_duplicates(
        &self,
        column: &str,
        values: &[(usize, &str)],
    ) -> Vec<ValidationViolation> {
        // first_seen maps value → first row it appeared on
        let mut first_seen: HashMap<&str, usize> = HashMap::new();
        let mut violations = Vec::new();

        for (row, value) in values {
            if value.trim().is_empty() {
                // Nulls are handled by NotNull; skip here.
                continue;
            }
            match first_seen.get(*value) {
                Some(&first_row) => {
                    violations.push(ValidationViolation {
                        rule: "no_duplicates".to_string(),
                        column: column.to_string(),
                        row: *row,
                        value: value.to_string(),
                        message: format!(
                            "Duplicate value '{}' in column '{}' at row {} \
                             (first seen at row {})",
                            value, column, row, first_row
                        ),
                    });
                }
                None => {
                    first_seen.insert(value, *row);
                }
            }
        }

        violations
    }

    fn check_regex(
        &mut self,
        column: &str,
        values: &[(usize, &str)],
        pattern: &str,
        custom_message: Option<&str>,
    ) -> Result<Vec<ValidationViolation>> {
        // Compile + cache the pattern.
        if !self.regex_cache.contains_key(pattern) {
            let re = Regex::new(pattern)?;
            self.regex_cache.insert(pattern.to_string(), re);
        }
        let re = self.regex_cache.get(pattern).expect("just inserted");

        Ok(values
            .iter()
            .filter(|(_, v)| !v.trim().is_empty() && !re.is_match(v))
            .map(|(row, v)| ValidationViolation {
                rule: "regex".to_string(),
                column: column.to_string(),
                row: *row,
                value: v.to_string(),
                message: custom_message
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| {
                        format!(
                            "Value '{}' in column '{}' at row {} does not match \
                             required pattern '{}'",
                            v, column, row, pattern
                        )
                    }),
            })
            .collect())
    }

    fn check_min_length(
        &self,
        column: &str,
        values: &[(usize, &str)],
        min: usize,
    ) -> Vec<ValidationViolation> {
        values
            .iter()
            .filter(|(_, v)| !v.trim().is_empty() && v.chars().count() < min)
            .map(|(row, v)| ValidationViolation {
                rule: "min_length".to_string(),
                column: column.to_string(),
                row: *row,
                value: v.to_string(),
                message: format!(
                    "Value '{}' in column '{}' at row {} has length {} \
                     which is below the minimum of {}",
                    v,
                    column,
                    row,
                    v.chars().count(),
                    min
                ),
            })
            .collect()
    }

    fn check_max_length(
        &self,
        column: &str,
        values: &[(usize, &str)],
        max: usize,
    ) -> Vec<ValidationViolation> {
        values
            .iter()
            .filter(|(_, v)| v.chars().count() > max)
            .map(|(row, v)| ValidationViolation {
                rule: "max_length".to_string(),
                column: column.to_string(),
                row: *row,
                value: v.to_string(),
                message: format!(
                    "Value '{}' in column '{}' at row {} has length {} \
                     which exceeds the maximum of {}",
                    v,
                    column,
                    row,
                    v.chars().count(),
                    max
                ),
            })
            .collect()
    }

    fn check_allowed_values(
        &self,
        column: &str,
        values: &[(usize, &str)],
        allowed: &[String],
    ) -> Vec<ValidationViolation> {
        let allowed_set: HashSet<&str> = allowed.iter().map(String::as_str).collect();

        values
            .iter()
            .filter(|(_, v)| !v.trim().is_empty() && !allowed_set.contains(*v))
            .map(|(row, v)| ValidationViolation {
                rule: "allowed_values".to_string(),
                column: column.to_string(),
                row: *row,
                value: v.to_string(),
                message: format!(
                    "Value '{}' in column '{}' at row {} is not in the \
                     allowed set: [{}]",
                    v,
                    column,
                    row,
                    allowed.join(", ")
                ),
            })
            .collect()
    }

    fn check_integer(
        &self,
        column: &str,
        values: &[(usize, &str)],
    ) -> Vec<ValidationViolation> {
        values
            .iter()
            .filter(|(_, v)| !v.trim().is_empty() && v.trim().parse::<i64>().is_err())
            .map(|(row, v)| ValidationViolation {
                rule: "integer".to_string(),
                column: column.to_string(),
                row: *row,
                value: v.to_string(),
                message: format!(
                    "Value '{}' in column '{}' at row {} is not a valid integer",
                    v, column, row
                ),
            })
            .collect()
    }

    fn check_numeric_range(        &self,
        column: &str,
        values: &[(usize, &str)],
        min: Option<f64>,
        max: Option<f64>,
    ) -> Vec<ValidationViolation> {
        values
            .iter()
            .filter_map(|(row, v)| {
                if v.trim().is_empty() {
                    return None;
                }
                match v.trim().parse::<f64>() {
                    Err(_) => Some(ValidationViolation {
                        rule: "numeric_range".to_string(),
                        column: column.to_string(),
                        row: *row,
                        value: v.to_string(),
                        message: format!(
                            "Value '{}' in column '{}' at row {} \
                             is not a valid number",
                            v, column, row
                        ),
                    }),
                    Ok(num) => {
                        if min.is_some_and(|m| num < m) {
                            Some(ValidationViolation {
                                rule: "numeric_range".to_string(),
                                column: column.to_string(),
                                row: *row,
                                value: v.to_string(),
                                message: format!(
                                    "Value {} in column '{}' at row {} \
                                     is below the minimum of {}",
                                    num,
                                    column,
                                    row,
                                    min.unwrap()
                                ),
                            })
                        } else if max.is_some_and(|m| num > m) {
                            Some(ValidationViolation {
                                rule: "numeric_range".to_string(),
                                column: column.to_string(),
                                row: *row,
                                value: v.to_string(),
                                message: format!(
                                    "Value {} in column '{}' at row {} \
                                     exceeds the maximum of {}",
                                    num,
                                    column,
                                    row,
                                    max.unwrap()
                                ),
                            })
                        } else {
                            None
                        }
                    }
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ValidationRule;

    fn engine() -> RuleEngine {
        RuleEngine::new()
    }

    fn pairs<'a>(data: &[(&'a str, &'a str)]) -> Vec<(usize, &'a str)> {
        data.iter()
            .enumerate()
            .map(|(i, (_, v))| (i + 2, *v))
            .collect()
    }

    // --- not_null -----------------------------------------------------------

    #[test]
    fn not_null_passes_on_non_empty() {
        let vals = pairs(&[("", "hello"), ("", "world")]);
        let violations = engine().apply(&ValidationRule::NotNull, "col", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn not_null_catches_empty() {
        let vals = pairs(&[("", "hello"), ("", ""), ("", "world"), ("", "")]);
        let violations = engine().apply(&ValidationRule::NotNull, "col", &vals);
        assert_eq!(violations.len(), 2);
        assert!(violations.iter().all(|v| v.rule == "not_null"));
    }

    #[test]
    fn not_null_catches_whitespace_only() {
        let vals = vec![(2, "   ")];
        let violations = engine().apply(&ValidationRule::NotNull, "col", &vals);
        assert_eq!(violations.len(), 1);
    }

    // --- no_duplicates ------------------------------------------------------

    #[test]
    fn no_duplicates_passes_unique() {
        let vals = pairs(&[("", "a"), ("", "b"), ("", "c")]);
        let violations = engine().apply(&ValidationRule::NoDuplicates, "col", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn no_duplicates_catches_duplicate() {
        let vals = pairs(&[("", "a"), ("", "b"), ("", "a")]);
        let violations = engine().apply(&ValidationRule::NoDuplicates, "col", &vals);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].value, "a");
    }

    #[test]
    fn no_duplicates_skips_nulls() {
        let vals = vec![(2, ""), (3, ""), (4, "unique")];
        let violations = engine().apply(&ValidationRule::NoDuplicates, "col", &vals);
        assert!(violations.is_empty());
    }

    // --- regex --------------------------------------------------------------

    #[test]
    fn regex_passes_matching_value() {
        let vals = pairs(&[("", "ORD-000001"), ("", "ORD-999999")]);
        let rule = ValidationRule::Regex {
            pattern: r"^ORD-\d{6}$".to_string(),
            message: None,
        };
        let violations = engine().apply(&rule, "order_id", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn regex_catches_non_matching_value() {
        let vals = pairs(&[("", "INVALID"), ("", "ORD-000001")]);
        let rule = ValidationRule::Regex {
            pattern: r"^ORD-\d{6}$".to_string(),
            message: None,
        };
        let violations = engine().apply(&rule, "order_id", &vals);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].value, "INVALID");
    }

    #[test]
    fn regex_skips_empty_values() {
        let vals = vec![(2, "")];
        let rule = ValidationRule::Regex {
            pattern: r"^ORD-\d{6}$".to_string(),
            message: None,
        };
        let violations = engine().apply(&rule, "order_id", &vals);
        assert!(violations.is_empty());
    }

    // --- allowed_values -----------------------------------------------------

    #[test]
    fn allowed_values_passes_valid() {
        let allowed = vec!["pending".to_string(), "shipped".to_string()];
        let vals = pairs(&[("", "pending"), ("", "shipped")]);
        let rule = ValidationRule::AllowedValues { values: allowed };
        let violations = engine().apply(&rule, "status", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn allowed_values_catches_invalid() {
        let allowed = vec!["pending".to_string(), "shipped".to_string()];
        let vals = pairs(&[("", "unknown"), ("", "pending")]);
        let rule = ValidationRule::AllowedValues { values: allowed };
        let violations = engine().apply(&rule, "status", &vals);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].value, "unknown");
    }

    // --- numeric_range ------------------------------------------------------

    #[test]
    fn numeric_range_passes_within_bounds() {
        let vals = pairs(&[("", "5.0"), ("", "10.0"), ("", "99.99")]);
        let rule = ValidationRule::NumericRange {
            min: Some(0.0),
            max: Some(100.0),
        };
        let violations = engine().apply(&rule, "amount", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn numeric_range_catches_below_min() {
        let vals = pairs(&[("", "-1.0"), ("", "5.0")]);
        let rule = ValidationRule::NumericRange {
            min: Some(0.0),
            max: None,
        };
        let violations = engine().apply(&rule, "amount", &vals);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn numeric_range_catches_non_numeric() {
        let vals = pairs(&[("", "not_a_number")]);
        let rule = ValidationRule::NumericRange {
            min: None,
            max: None,
        };
        let violations = engine().apply(&rule, "amount", &vals);
        assert_eq!(violations.len(), 1);
    }

    // --- min/max length -----------------------------------------------------

    #[test]
    fn min_length_passes_long_enough() {
        let vals = pairs(&[("", "hello")]);
        let rule = ValidationRule::MinLength { value: 3 };
        assert!(engine().apply(&rule, "col", &vals).is_empty());
    }

    #[test]
    fn min_length_catches_too_short() {
        let vals = pairs(&[("", "hi")]);
        let rule = ValidationRule::MinLength { value: 5 };
        assert_eq!(engine().apply(&rule, "col", &vals).len(), 1);
    }

    #[test]
    fn max_length_catches_too_long() {
        let vals = pairs(&[("", "toolongstring")]);
        let rule = ValidationRule::MaxLength { value: 5 };
        assert_eq!(engine().apply(&rule, "col", &vals).len(), 1);
    }

    // --- integer ------------------------------------------------------------

    #[test]
    fn integer_passes_valid_integers() {
        let vals = pairs(&[("", "1"), ("", "42"), ("", "-7"), ("", "0")]);
        let violations = engine().apply(&ValidationRule::Integer, "id", &vals);
        assert!(violations.is_empty());
    }

    #[test]
    fn integer_catches_float() {
        let vals = pairs(&[("", "3.14")]);
        let violations = engine().apply(&ValidationRule::Integer, "id", &vals);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "integer");
    }

    #[test]
    fn integer_catches_text() {
        let vals = pairs(&[("", "hello")]);
        let violations = engine().apply(&ValidationRule::Integer, "id", &vals);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn integer_skips_empty_values() {
        let vals = vec![(2, "")];
        let violations = engine().apply(&ValidationRule::Integer, "id", &vals);
        assert!(violations.is_empty());
    }
}
