//! Build version, sourced from the repository's `VERSION` file so the Rust
//! binary reports the same value the Go gateway did.

/// The current HeiGate version (e.g. `"2.0.0"`).
pub const VERSION: &str = include_str!("../../../VERSION");
