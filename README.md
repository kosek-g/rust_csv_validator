# csv_validator

A production-ready CLI tool for validating CSV files stored in **Azure Data Lake Storage Gen2 (ADLS Gen2)**. Define your data quality rules in a YAML config file and run validation on a schedule via GitHub Actions — or on demand from the command line.

## Features

- **7 built-in rule types**: `not_null`, `no_duplicates`, `regex`, `min_length`, `max_length`, `allowed_values`, `numeric_range`, `integer`
- **Two run modes**: cloud (Azure ADLS Gen2) or local file (no credentials needed)
- **Three output formats**: human-readable coloured table, JSON (for CI pipelines), single-line summary (for scripts)
- **Fail-fast mode**: stop on the first failing file
- **Dry-run mode**: validate config syntax without touching any data
- **GitHub Actions workflow** included — runs on a cron schedule, uploads a JSON report artifact
- **Zero unsafe code**, structured logging, fully tested (34 unit tests + 5 integration tests)

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.75 or later
- Azure Storage Account key (for Azure mode)

### Install

```bash
git clone https://github.com/kosek-g/rust_csv_validator.git
cd rust_csv_validator
cargo install --path .
```

### Local file validation (no Azure required)

```bash
csv_validator \
  --local-file /path/to/data.csv \
  --config config/example_config.yaml
```

### Azure ADLS Gen2 validation

```bash
export AZURE_STORAGE_KEY="your_storage_account_key"

csv_validator \
  --account  mystorageaccount \
  --container raw-data \
  --config config/my_rules.yaml
```

## CLI Reference

```
Usage: csv_validator [OPTIONS] --config <FILE>

Options:
  -c, --container <CONTAINER>  Azure Storage container name
  -l, --local-file <PATH>      Path to a local CSV file (skips Azure)
  -C, --config <FILE>          Path to the YAML validation config file [env: CSV_VALIDATOR_CONFIG]
      --account <ACCOUNT>      Azure Storage account name [env: AZURE_STORAGE_ACCOUNT]
  -o, --output <FORMAT>        Output format: pretty | json | summary [default: pretty]
      --fail-fast              Stop on first validation failure
      --dry-run                Validate config syntax only — no network calls
  -v, --verbose                Increase log verbosity (-v info, -vv debug, -vvv trace)
  -h, --help                   Print help
  -V, --version                Print version
```

Environment variables take precedence over config file values; CLI flags take precedence over both.

## Configuration File

Rules are defined in YAML. The config has three sections:

| Section | Purpose |
|---|---|
| `storage` | Optional: Azure account name (can also be set via CLI or env var) |
| `files` | Which paths inside the container to scan, and with what filename glob |
| `validations` | Column-level rules, matched to files by path prefix |

### Example

```yaml
storage:
  account: "mystorageaccount"

files:
  - path: "raw/orders"
    file_pattern: "*.csv"
  - path: "raw/events"
    file_pattern: "events_*.csv"

validations:
  - file_pattern: "raw/orders"
    columns:
      - name: "order_id"
        rules:
          - type: not_null
          - type: regex
            pattern: '^ORD-[0-9]{6}$'
            message: "Order ID must match ORD-XXXXXX"

      - name: "amount"
        rules:
          - type: numeric_range
            min: 0.01
            max: 100000.0

      - name: "status"
        rules:
          - type: allowed_values
            values: ["pending", "shipped", "delivered", "cancelled"]
```

### All Rule Types

| Rule | Required fields | Description |
|---|---|---|
| `not_null` | — | Value must not be empty or whitespace |
| `no_duplicates` | — | No two rows may share the same value |
| `regex` | `pattern`, optional `message` | Value must match the regex |
| `min_length` | `value` | String length ≥ value |
| `max_length` | `value` | String length ≤ value |
| `allowed_values` | `values` (list) | Value must be in the list |
| `numeric_range` | optional `min`, optional `max` | Parseable float within bounds |
| `integer` | — | Value must be parseable as a whole number |

Empty cells are **skipped** by all rules except `not_null`.

## Output Formats

### Pretty (default)

```
════════════════════════════════════════════════════════════════════════
  CSV DATA QUALITY VALIDATION REPORT
════════════════════════════════════════════════════════════════════════
  Container : rust
  Duration  : 0.42s
  Files     : 1
  Status    : FAILED
────────────────────────────────────────────────────────────────────────
  ✗  logs.csv  (3 violation(s))
       → [regex] row    5 — timestamp must be ISO 8601 format
       → [integer] row  12 — 'id' must be an integer, got 'N/A'
```

### JSON (`--output json`)

Machine-readable, suitable for downstream processing. Includes full violation details per file.

### Summary (`--output summary`)

```
container=rust files=1 violations=3 status=FAIL
```

## GitHub Actions

The included workflow (`.github/workflows/csv_validation.yml`) runs validation on a daily schedule and on manual trigger.

### Required secrets

| Secret | Description |
|---|---|
| `AZURE_STORAGE_ACCOUNT` | Storage account name |
| `AZURE_STORAGE_KEY` | Storage account access key |

### Required repository variable

| Variable | Description |
|---|---|
| `CSV_VALIDATOR_CONTAINER` | Default container to validate (overridable at runtime) |

### Workflow steps

1. Checkout → install Rust → restore cache
2. `cargo build --release`
3. `cargo test` (all 39 tests)
4. `cargo clippy -- -D warnings`
5. Dry-run config validation
6. Live ADLS validation (pretty output)
7. Re-run as JSON → upload as downloadable artifact (kept 30 days)

### Manual trigger

Go to **Actions → CSV Data Quality Validation → Run workflow** and optionally override the container name.

## Project Structure

```
csv_validator/
├── src/
│   ├── main.rs        # Entry point, two run modes (local / Azure)
│   ├── lib.rs         # Re-exports all modules for integration tests
│   ├── cli.rs         # All CLI argument definitions (clap derive)
│   ├── config.rs      # YAML config parsing + semantic validation
│   ├── rules.rs       # Rule engine + all 8 rule implementations
│   ├── validator.rs   # File-level orchestration (CSV → rules → results)
│   ├── storage.rs     # Azure ADLS Gen2 client (object_store)
│   ├── reporter.rs    # Report rendering (pretty / JSON / summary)
│   └── error.rs       # AppError enum (thiserror)
├── config/
│   ├── example_config.yaml      # Annotated reference config
│   └── adls_logs_config.yaml    # Live config for genaidemoava/rust
├── tests/
│   ├── integration_tests.rs
│   └── fixtures/
│       ├── test_config.yaml
│       ├── valid_orders.csv
│       └── invalid_orders.csv
├── .github/workflows/
│   └── csv_validation.yml
└── .env.example
```

## Development

```bash
# Run all tests
cargo test

# Lint
cargo clippy -- -D warnings

# Dry-run a config file
./target/debug/csv_validator --container x --config config/example_config.yaml --dry-run

# Validate a local file
./target/debug/csv_validator --local-file logs.csv --config config/adls_logs_config.yaml
```

## License

MIT
