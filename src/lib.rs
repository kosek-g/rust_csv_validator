//! `csv_validator` library crate.
//!
//! Exposes the public API used by integration tests and any downstream code
//! that embeds the validation engine.

pub mod config;
pub mod error;
pub mod reporter;
pub mod rules;
pub mod storage;
pub mod validator;
