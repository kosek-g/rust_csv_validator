//! Validation report formatting.
//!
//! `ValidationReport` owns the aggregated results from all validated files.
//! It provides three output renderers: pretty, JSON, and summary.

use std::time::Duration;

use colored::*;

use crate::validator::FileValidationResult;

// ---------------------------------------------------------------------------
// ValidationReport
// ---------------------------------------------------------------------------

/// Aggregated results for an entire validation run.
pub struct ValidationReport {
    /// Per-file results.
    pub results: Vec<FileValidationResult>,
    /// Container name (used in report headings and email subjects).
    pub container: String,
    /// Wall-clock time the run took.
    pub duration: Duration,
}

impl ValidationReport {
    /// `true` when every file passed without any violations.
    pub fn is_valid(&self) -> bool {
        self.results.iter().all(FileValidationResult::is_valid)
    }

    /// Total number of violations across all files.
    pub fn total_violations(&self) -> usize {
        self.results.iter().map(|r| r.violations.len()).sum()
    }

    /// Subset of results that contain at least one violation.
    pub fn failed_files(&self) -> Vec<&FileValidationResult> {
        self.results
            .iter()
            .filter(|r| !r.is_valid())
            .collect()
    }

    // ------------------------------------------------------------------
    // Output renderers
    // ------------------------------------------------------------------

    /// Print a coloured, human-readable report to **stdout**.
    pub fn print_pretty(&self) {
        let border = "═".repeat(72);
        let sep = "─".repeat(72);

        println!("\n{}", border.bright_blue());
        println!("{}", "  CSV DATA QUALITY VALIDATION REPORT".bold());
        println!("{}", border.bright_blue());
        println!("  Container : {}", self.container.cyan());
        println!(
            "  Duration  : {:.2}s",
            self.duration.as_secs_f64()
        );
        println!("  Files     : {}", self.results.len());
        println!(
            "  Status    : {}",
            if self.is_valid() {
                "PASSED".green().bold()
            } else {
                "FAILED".red().bold()
            }
        );
        println!("{}", sep.dimmed());

        for result in &self.results {
            let short_name = result
                .file_path
                .rsplit('/')
                .next()
                .unwrap_or(&result.file_path);

            if result.is_valid() {
                println!(
                    "  {}  {}  ({} rows · {} columns)",
                    "✓".green().bold(),
                    short_name.green(),
                    result.rows_checked,
                    result.columns_checked,
                );
            } else {
                println!(
                    "  {}  {}  ({} violation(s))",
                    "✗".red().bold(),
                    short_name.red().bold(),
                    result.violations.len(),
                );
                for v in &result.violations {
                    println!(
                        "       {} [{}] row {:>4} — {}",
                        "→".yellow(),
                        v.rule.yellow(),
                        v.row,
                        v.message,
                    );
                }
            }
        }

        println!("{}", border.bright_blue());
        if self.is_valid() {
            println!(
                "  {}  All {} file(s) passed validation.\n",
                "✓".green().bold(),
                self.results.len()
            );
        } else {
            println!(
                "  {}  {} violation(s) across {} file(s) — see details above.\n",
                "✗".red().bold(),
                self.total_violations(),
                self.failed_files().len(),
            );
        }
    }

    /// Print a machine-readable JSON report to **stdout**.
    pub fn print_json(&self) {
        let report = serde_json::json!({
            "container": self.container,
            "duration_secs": self.duration.as_secs_f64(),
            "valid": self.is_valid(),
            "total_violations": self.total_violations(),
            "files_checked": self.results.len(),
            "results": self.results.iter().map(|r| {
                serde_json::json!({
                    "path": r.file_path,
                    "valid": r.is_valid(),
                    "rows_checked": r.rows_checked,
                    "columns_checked": r.columns_checked,
                    "violations": r.violations.iter().map(|v| {
                        serde_json::json!({
                            "rule":    v.rule,
                            "column":  v.column,
                            "row":     v.row,
                            "value":   v.value,
                            "message": v.message,
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });

        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .expect("ValidationReport is always JSON-serialisable")
        );
    }

    /// Print a single-line summary to **stdout** (suitable for scripts).
    pub fn print_summary(&self) {
        println!(
            "container={} files={} violations={} status={}",
            self.container,
            self.results.len(),
            self.total_violations(),
            if self.is_valid() { "PASS" } else { "FAIL" },
        );
    }

}
