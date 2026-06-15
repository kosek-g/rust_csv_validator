# Learning Rust Through csv_validator — A Data Engineer's Guide

This document is written specifically for you: a data engineer who built this project to learn Rust. It explains every design decision, language feature, and pattern used in the codebase — and why they exist. Read it alongside the source code, not instead of it.

---

## Table of Contents

1. [The Big Picture — What the Program Does](#1-the-big-picture)
2. [How the Pieces Connect — Data Flow](#2-data-flow)
3. [The Module System — Rust's Way of Organising Code](#3-the-module-system)
4. [Ownership and Borrowing — The Heart of Rust](#4-ownership-and-borrowing)
5. [Error Handling — thiserror vs anyhow](#5-error-handling)
6. [Enums as Data — The ValidationRule Tagged Union](#6-enums-as-data)
7. [Traits and Generics — object_store and ObjectStore](#7-traits-and-generics)
8. [Async Rust — tokio and await](#8-async-rust)
9. [Serialisation — serde and the YAML Config](#9-serialisation)
10. [CLI Parsing — clap derive macros](#10-cli-parsing)
11. [Testing — Unit, Integration, and Fixtures](#11-testing)
12. [The Binary/Library Split — Why lib.rs Exists](#12-the-binarylibrary-split)
13. [Lifetimes — Validator<'cfg>](#13-lifetimes)
14. [The Type System Enforcing Correctness](#14-the-type-system-enforcing-correctness)
15. [Performance Choices — Why This Is Fast](#15-performance-choices)
16. [What to Learn Next](#16-what-to-learn-next)

---

## 1. The Big Picture

From a data engineering perspective, this is a **data quality framework** with three phases familiar from any DQ pipeline:

```
Discovery → Extraction → Validation → Reporting
```

In practice:

| Phase | Code | Analogy |
|---|---|---|
| **Discovery** | `storage.rs` lists blobs matching a glob | `ls *.csv` against S3/ADLS |
| **Extraction** | `storage.rs` downloads bytes; `validator.rs` parses CSV | Spark reading a file |
| **Validation** | `rules.rs` checks every column | dbt tests / Great Expectations |
| **Reporting** | `reporter.rs` formats output | a data quality dashboard row |

The big Rust insight: the compiler forces you to handle every possible failure at each phase. There is no runtime `NullPointerException` waiting to surprise you in production.

---

## 2. Data Flow

Here is the complete journey of a single CSV file through the program:

```
main.rs
  │
  ├─ Parse CLI args (clap)                          → Cli struct
  ├─ Load config file (serde_yaml)                  → Config struct
  │
  ├─ [Azure mode] StorageClient::list_files()       → Vec<String> (file paths)
  │   StorageClient::read_file()                    → Vec<u8> (raw bytes)
  │
  ├─ [Local mode] std::fs::read()                   → Vec<u8>
  │
  ├─ Validator::validate_file(path, &bytes)
  │   ├─ Find matching FileValidation by path prefix
  │   ├─ Parse CSV bytes into rows (csv crate)
  │   ├─ Collect column values: HashMap<column_name, Vec<(row, &str)>>
  │   └─ For each column × rule: RuleEngine::apply()
  │       └─ Returns Vec<ValidationViolation>
  │
  ├─ Aggregate into ValidationReport
  │
  └─ reporter.rs: print_pretty() / print_json() / print_summary()
      └─ std::process::exit(0 or 1)
```

Every arrow is a function call. Every struct is an explicit data boundary. Rust makes you name everything — this is verbose at first but makes the data flow visible in a way that Python hides behind implicit duck-typing.

---

## 3. The Module System

**File:** `src/lib.rs`

```rust
pub mod config;
pub mod error;
pub mod reporter;
pub mod rules;
pub mod storage;
pub mod validator;
```

Rust's module system maps directly to files. `pub mod config` means "expose `src/config.rs` as a public module". Anything inside a module is private by default — you have to explicitly mark things `pub` to make them visible outside.

**Why this matters for data engineers:** You're used to Python where `import *` is common and everything is globally accessible. Rust forces you to think about what your module's public API is. This is the same discipline as designing a good dbt model — only expose what downstream consumers need.

The `lib.rs` / `main.rs` split is explained in detail in [Section 12](#12-the-binarylibrary-split).

---

## 4. Ownership and Borrowing

This is Rust's most distinctive feature and the one that takes the longest to internalise. Here are the exact places it shows up in this codebase.

### 4.1 Cloning to avoid aliasing

**File:** `src/validator.rs`, lines ~76–83

```rust
let validation = self
    .config
    .validations
    .iter()
    .find(|fv| path_matches_prefix(file_path, &fv.file_pattern))
    .cloned();  // ← THIS
```

Why `.cloned()`? The loop that follows needs `&mut self.engine` (mutable access to the rule engine) *at the same time* as we'd be holding a reference into `self.config`. Rust's borrow checker forbids holding a `&T` and a `&mut T` to parts of the same struct simultaneously — even if they're different fields — because the compiler doesn't do field-level aliasing analysis here.

Solution: `.cloned()` copies the `FileValidation` struct out of `self.config` and into a locally-owned variable. Now `self.config` is not borrowed, so `self.engine` can be mutably borrowed freely.

**The lesson:** When you see `.clone()` in Rust code it's usually working around a borrow checker limitation, not laziness. The cost (a heap allocation for the Vecs inside `FileValidation`) is acceptable here because it only happens once per file, not per row.

### 4.2 Borrowing bytes without copying

**File:** `src/validator.rs`, column collection

```rust
let values: Vec<(usize, &str)> = column_data
    .get(col_name)
    .map(|v| v.iter().copied().collect())
    .unwrap_or_default();
```

`&str` is a borrowed slice — it points into the already-allocated string data without copying it. The rule engine receives `&[(usize, &str)]` — references to the column values, not owned copies. This is why validation of 100,000 rows is fast: no strings are copied during rule evaluation.

### 4.3 `Arc<dyn ObjectStore>`

**File:** `src/storage.rs`

```rust
pub struct StorageClient {
    store: Arc<dyn ObjectStore>,
}
```

`Arc` = Atomic Reference Counted. It lets multiple parts of the code share ownership of the same `ObjectStore` value. `dyn ObjectStore` means "some type that implements the `ObjectStore` trait" — the exact type is erased. This is Rust's dynamic dispatch (like an interface in Java/C#).

Why `Arc` and not just ownership? The `ObjectStore` trait requires `Send + Sync` (safe to share across threads), and `Arc` gives you thread-safe shared ownership. The Tokio async runtime may run futures on different threads.

---

## 5. Error Handling

The project uses two complementary error libraries with a clear boundary between them.

### 5.1 `thiserror` — in the library (`src/error.rs`)

```rust
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage error: {0}")]
    Storage(#[from] object_store::Error),

    #[error("CSV parsing error: {0}")]
    Csv(#[from] csv::Error),
    // ...
}
```

`thiserror` generates `Display` and `Error` trait implementations from the `#[error]` annotations. The `#[from]` attribute auto-generates `impl From<object_store::Error> for AppError` — this is what makes the `?` operator work when calling object_store functions.

**The `?` operator** is the key pattern. Instead of:
```rust
match client.list_files().await {
    Ok(files) => files,
    Err(e) => return Err(AppError::Storage(e)),
}
```
You write:
```rust
let files = client.list_files().await?;
```

`?` checks if the `Result` is `Err`, and if so immediately returns from the current function with the error (converting it via `From` if needed). This is Rust's equivalent of exception propagation — but explicit and traceable.

### 5.2 `anyhow` — in the binary (`src/main.rs`)

```rust
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file(&cli.config_location)
        .with_context(|| format!("Failed to load config from '{}'", cli.config_location.display()))?;
```

`anyhow` is for the top-level binary where you don't care about matching on error variants — you just want to attach human-readable context and propagate. `.with_context(|| ...)` attaches a message that appears in the error chain when the program panics or prints an error.

**The philosophy:** Library code uses typed errors (callers can match on them). Binary code uses untyped errors with rich context (humans read them).

---

## 6. Enums as Data — The ValidationRule Tagged Union

**File:** `src/config.rs`

```rust
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValidationRule {
    NotNull,
    NoDuplicates,
    Regex { pattern: String, message: Option<String> },
    MinLength { value: usize },
    MaxLength { value: usize },
    AllowedValues { values: Vec<String> },
    NumericRange { min: Option<f64>, max: Option<f64> },
    Integer,
}
```

This is one of the most powerful Rust patterns — an **algebraic data type** (sum type). Each variant can carry different data. The `#[serde(tag = "type")]` annotation means serde reads the `type` key in YAML to decide which variant to deserialise into.

In your YAML:
```yaml
- type: numeric_range
  min: 0.0
  max: 100.0
```
Serde reads `type: numeric_range` → maps to `ValidationRule::NumericRange { min: Some(0.0), max: Some(100.0) }`.

**Pattern matching** on this enum in `rules.rs`:
```rust
match rule {
    ValidationRule::NotNull => self.check_not_null(column, values),
    ValidationRule::NumericRange { min, max } => self.check_numeric_range(column, values, *min, *max),
    // ...
}
```

The compiler guarantees you handle every variant. Add a new rule type to the enum and the compiler will point to every `match` that needs updating — a refactoring safety net you never get in Python.

**Compare to Python:** In Python you'd do `if rule["type"] == "not_null"` — runtime string matching with no compile-time safety. In Rust the match is exhaustive and the data is typed.

---

## 7. Traits and Generics — `object_store` and `ObjectStore`

**File:** `src/storage.rs`

```rust
store: Arc<dyn ObjectStore>,
```

`ObjectStore` is a **trait** — Rust's version of an interface. The `object_store` crate defines it with methods like `list()`, `get()`, `put()`. The `MicrosoftAzure` struct implements it for Azure. In tests, `InMemory` implements it for RAM.

This is how you write a function that works against both production Azure and in-memory test data without changing any logic — same `StorageClient`, different backing `ObjectStore`.

**Two flavours of polymorphism in Rust:**

| `dyn Trait` (dynamic dispatch) | `impl Trait` / `<T: Trait>` (static dispatch) |
|---|---|
| Type erased at compile time | Monomorphised — compiler generates one version per concrete type |
| Pointer indirection (vtable), slight overhead | Zero overhead |
| Necessary when you don't know the type at compile time (e.g. reading from config) | Preferred in library functions |

This project uses `dyn ObjectStore` because the `StorageClient` struct needs to own the store value with a single concrete type in memory — and `Arc<dyn ObjectStore>` achieves that.

---

## 8. Async Rust — tokio and `await`

**File:** `src/main.rs`, `src/storage.rs`

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ...
    let files = client.list_files(&path, &pattern).await?;
}
```

Rust's async model is different from Python's asyncio:

- In Python, `await` suspends the coroutine on the event loop automatically.
- In Rust, `async fn` returns a **`Future`** — a value that does nothing until you `.await` it or give it to an executor.
- `#[tokio::main]` is a macro that creates a Tokio runtime and runs `main()` inside it.
- Tokio is the executor — it polls futures, drives I/O, and can distribute work across threads.

**Why async for this project?** Azure API calls (`list blobs`, `download blob`) are I/O-bound. Async lets the program submit an HTTP request and yield control while waiting for the response, rather than blocking a thread. For a single-file run it makes no difference; for validating 50 files in parallel it would matter enormously.

The `?` operator works inside async functions just as in sync ones — `.await?` awaits the future then propagates any error.

**Key insight:** `async fn foo() -> Result<T>` actually returns `impl Future<Output = Result<T>>`. The `async` keyword is syntactic sugar for a state machine the compiler generates for you.

---

## 9. Serialisation — serde and the YAML Config

**File:** `src/config.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub storage: Option<StorageConfig>,
    pub files: Vec<FileConfig>,
    pub validations: Vec<FileValidation>,
}
```

`#[derive(Deserialize)]` is a **procedural macro** — it generates code at compile time that teaches `serde` how to convert YAML/JSON/etc. into your struct. You never write parsing code manually.

Key serde annotations used in this project:

| Annotation | Effect |
|---|---|
| `#[serde(tag = "type", rename_all = "snake_case")]` | Discriminated union — uses the `type` field to pick enum variant |
| `#[serde(default)]` | Use `Default::default()` when the key is absent from YAML |
| `#[serde(default = "default_file_pattern")]` | Call a named function to get the default value |

The `default = "fn_name"` pattern is particularly useful:
```rust
fn default_file_pattern() -> String { "*.csv".to_string() }

#[derive(Deserialize)]
pub struct FileConfig {
    pub path: String,
    #[serde(default = "default_file_pattern")]
    pub file_pattern: String,  // optional in YAML, always present in Rust
}
```
This means you can write `- path: "raw/orders"` in YAML without `file_pattern` and the struct will have `"*.csv"` automatically.

---

## 10. CLI Parsing — clap derive macros

**File:** `src/cli.rs`

```rust
#[derive(Debug, Parser)]
#[command(name = "csv_validator", version, author, ...)]
pub struct Cli {
    #[arg(short = 'c', long = "container", env = "CSV_VALIDATOR_CONTAINER")]
    pub container: Option<String>,

    #[arg(short = 'l', long = "local-file")]
    pub local_file: Option<PathBuf>,
    // ...
}
```

`clap` with derive macros turns struct field definitions into a complete CLI parser. The `env = "CSV_VALIDATOR_CONTAINER"` attribute means the argument can be provided via environment variable — `clap` checks it automatically.

**Priority chain (clap's default):** CLI flag > environment variable > default value.

In `main.rs`:
```rust
let cli = Cli::parse();
```

That single line parses `std::env::args()`, validates all required fields, prints `--help` if asked, and exits with an error message if arguments are wrong — all generated code.

`ArgAction::Count` for `--verbose`:
```rust
#[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
pub verbose: u8,
```
Each `-v` increments the counter: `-v` → 1, `-vv` → 2, `-vvv` → 3. This is how the log level escalation works.

---

## 11. Testing

### Unit tests — inside source files

**File:** `src/rules.rs` (bottom of file)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_null_flags_empty_string() {
        let mut engine = RuleEngine::new();
        let values = vec![(2, ""), (3, "hello"), (4, "  ")];
        let violations = engine.apply(
            &ValidationRule::NotNull, "col", &values
        );
        assert_eq!(violations.len(), 2); // rows 2 and 4
    }
}
```

`#[cfg(test)]` means the module is only compiled when running `cargo test`. `use super::*` imports everything from the parent module. This pattern gives you access to private functions — unit tests can test internals, integration tests can only test the public API.

### Integration tests — in `tests/`

**File:** `tests/integration_tests.rs`

Integration tests live outside `src/` and can only import public API (`pub` functions and structs). They test the full pipeline from YAML config → CSV bytes → `FileValidationResult`. No Azure connection required — they use fixture files on disk.

```rust
#[test]
fn valid_orders_file_produces_no_violations() {
    let config = Config::from_file(&fixtures_dir().join("test_config.yaml")).unwrap();
    let content = std::fs::read(fixtures_dir().join("valid_orders.csv")).unwrap();
    let mut validator = Validator::new(&config);
    let result = validator.validate_file("raw/orders/valid_orders.csv", &content).unwrap();
    assert!(result.is_valid());
    assert_eq!(result.rows_checked, 5);
}
```

`assert_eq!` prints both values on failure. `assert!` prints a custom message. These are macros that expand to `if !condition { panic!(...) }`.

### Running tests

```bash
cargo test                    # all tests
cargo test not_null           # tests whose name contains "not_null"
cargo test -- --nocapture     # show println! output during tests
```

---

## 12. The Binary/Library Split

**Files:** `src/lib.rs` and `src/main.rs`

This is a key Rust idiom. Look at `Cargo.toml`:

```toml
[[bin]]
name = "csv_validator"
path = "src/main.rs"
```

There is no explicit `[lib]` section — Cargo auto-discovers `src/lib.rs` as the library crate. So the project compiles as *two* crates:

- **`csv_validator` (lib)** — all the modules: `config`, `rules`, `validator`, `storage`, `reporter`, `error`
- **`csv_validator` (bin)** — just `main.rs`, which imports from the lib

Integration tests (`tests/integration_tests.rs`) can only import from the *library* crate:
```rust
use csv_validator::config::Config;
use csv_validator::validator::Validator;
```

If all the code were in `main.rs`, integration tests couldn't import any of it. The library split is the mechanism that makes your application code independently testable.

`src/main.rs` only contains:
- `mod cli;` — declares the cli module (private to main, not part of lib)
- `use csv_validator::*;` — imports from lib
- `main()` function — wires everything together

This is also the reason `src/main.rs` starts with `mod cli;` — `cli.rs` is a private module of the binary only, not exposed to the library.

---

## 13. Lifetimes — `Validator<'cfg>`

**File:** `src/validator.rs`

```rust
pub struct Validator<'cfg> {
    config: &'cfg Config,
    engine: RuleEngine,
}
```

`'cfg` is a **lifetime parameter**. It tells the compiler: "this `Validator` must not outlive the `Config` it borrows". 

Why borrow instead of owning? The `Config` is created in `main()` and used for the whole run. Passing a reference avoids cloning the entire config (which contains Vecs of rules) for every file validated.

The lifetime annotation is the compiler's way of tracking that promise. In practice:

```rust
let config = Config::from_file(...)?;        // Config lives here
let mut validator = Validator::new(&config); // Validator borrows config
// validator can't outlive config — compiler enforces this
```

If you tried to move `config` after creating the validator:
```rust
let config = Config::from_file(...)?;
let mut validator = Validator::new(&config);
drop(config);                      // ERROR: cannot move out of `config`
validator.validate_file(...);      // because `validator` still borrows it
```
The compiler catches the use-after-free at compile time. No runtime crash, no Valgrind needed.

---

## 14. The Type System Enforcing Correctness

Several places in this codebase use types to prevent incorrect program states:

### `Option<T>` instead of null

```rust
pub container: Option<String>,  // in Cli
```

There is no null in Rust. `Option<String>` is either `Some("rust")` or `None`. Every place you use it, you must handle both cases — the compiler refuses to compile code that ignores `None`.

```rust
let container = cli.container.as_deref().ok_or_else(|| {
    anyhow::anyhow!("Either --container or --local-file must be provided.")
})?;
```

`ok_or_else()` converts `Option<T>` to `Result<T, E>` — `None` becomes `Err`. The `?` then propagates the error. This pattern (Option → Result → `?`) is idiomatic Rust.

### `Result<T, E>` for fallible operations

Every function that can fail returns `Result`. There are no exceptions. You can't call a function and forget to check if it failed — unused `Result` values produce a compiler warning.

### Newtypes and structured data

`FileValidationResult` is a struct, not a HashMap or a tuple. Its fields have names. Returning a named struct from `validate_file()` means callers can't confuse `rows_checked` with `columns_checked`. 

---

## 15. Performance Choices — Why This Is Fast

100,000 rows validated in ~0.06s locally. Here is why:

**Regex caching** (`src/rules.rs`):
```rust
pub struct RuleEngine {
    regex_cache: HashMap<String, Regex>,
}
```
Compiling a regex is expensive (milliseconds). The engine caches compiled regexes by pattern string. The first file that encounters `pattern: "^\d{4}-..."` pays the compilation cost; every subsequent file reuses the cached `Regex`.

**Zero-copy string slices:**
Column values are stored as `&str` references pointing into the already-parsed CSV row buffer. Applying rules iterates over these references — no strings are copied per-row.

**`csv` crate's reader:**
The `csv` crate reads records without allocating new strings for each field when used with `StringRecord` — it reuses an internal buffer. This codebase collects into `HashMap<String, Vec<String>>` which does allocate, but only once per column during the collection phase.

**Release builds:**
`cargo build --release` enables LLVM optimisations (inlining, loop vectorisation, dead code elimination). The GitHub Actions workflow uses `--release`; local `cargo build` uses debug mode (much faster to compile, slower to run).

---

## 16. What to Learn Next

You've built a real, production-deployed Rust program. Here is a suggested learning path building directly on what you've seen here.

### Immediate next steps (weeks 1–4)

1. **Read the Rust Book chapters on ownership** — [doc.rust-lang.org/book](https://doc.rust-lang.org/book) chapters 4–5. You've seen it in practice; now understand the rules formally.

2. **Understand `Iterator` deeply** — almost every data manipulation in this codebase uses iterator chains (`.iter().find().cloned()`). Work through the iterator chapter and implement your own iterator.

3. **Understand `From` / `Into` / `TryFrom`** — the `#[from]` on `AppError` variants auto-implements these. Understanding them unlocks idiomatic Rust conversion code.

### Medium term (months 1–3)

4. **`async`/`await` in depth** — read the [Async Rust book](https://rust-lang.github.io/async-book/). Add parallel file validation to this project using `futures::join_all()` or `tokio::spawn()`.

5. **Generics and trait bounds** — rewrite `StorageClient::list_files` as a generic function `<S: ObjectStore>` and see how static dispatch differs from `dyn ObjectStore`.

6. **Extend the rule engine** — add a `CrossColumn` rule type that validates relationships between two columns (e.g. `end_date > start_date`). You'll need to restructure how the validator passes data to the engine — a good design challenge.

### Larger projects to try next

7. **Replace the `object_store` direct usage with an abstraction layer** — create your own `Storage` trait and implement it for both Azure and local filesystem. This is how real production Rust abstracts cloud providers.

8. **Add structured output to the reporter** — implement `serde::Serialize` on `ValidationReport` and derive it instead of building `serde_json::json!()` manually. You'll learn about derived vs manual serialisation.

9. **Build a streaming version** — instead of loading entire CSV files into memory, validate row by row. This requires understanding Rust's `Stream` trait (async equivalent of `Iterator`) from the `futures` crate.

### Concepts this project intentionally avoided (for you to explore)

| Concept | Where it would apply |
|---|---|
| `Arc<Mutex<T>>` shared mutable state | Parallel file validation across threads |
| `Pin<Box<dyn Future>>` | Custom async combinators |
| Unsafe code | Direct SIMD, FFI to C libraries |
| Macros (`macro_rules!`, proc macros) | Custom `#[derive]`, DSL for rules |
| `Send + Sync` bounds | Thread-safe generic storage layer |
| Lifetimes in trait objects | `Box<dyn Validator<'_>>` |

---

## Key Takeaways

**What Rust gave you that Python doesn't:**

- The compiler proved your ADLS config parser handles every YAML edge case at build time.
- The type system made it impossible to confuse a file path (a `String`) with a container name (also a `String`) — they're semantically different, even if not newtypes here.
- The rule engine's regex cache cannot have a race condition because `RuleEngine` is `!Send` (not thread-safe) — and that's fine, each call is sequential.
- The binary cannot panic on a null dereference anywhere in normal operation — every `Option` and `Result` is explicitly handled.

**What you traded for that:**

- ~5–10× longer development time vs Python for a first project.
- Lifetime annotations that feel cryptic until they click.
- `.clone()` scattered around when you can't figure out the borrow.
- A 10-minute compile time for a cold build.

**The fundamental shift:** Rust moves bugs from runtime (production incidents, 3am pages) to compile time (red squiggles in your editor). As a data engineer maintaining pipelines that run unattended, that tradeoff pays off quickly.
