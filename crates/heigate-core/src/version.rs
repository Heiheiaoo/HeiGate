//! Build version, sourced from the repository's `VERSION` file so the Rust
//! binary reports the same value the Go gateway did.

/// The current HeiGate version (e.g. `"1.0.5"`).
pub const VERSION: &str = include_str!("../../../VERSION");
