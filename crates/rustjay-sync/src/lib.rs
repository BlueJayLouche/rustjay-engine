//! rustjay-sync — Ableton Link and ProDJ Link tempo sync.
//!
//! **Note:** The `link` feature links against Ableton Link (GPL-2.0+); the resulting binary is subject to GPL terms.

#[cfg(feature = "link")]
pub mod link;
#[cfg(feature = "prodj")]
pub mod prodj;

#[cfg(feature = "link")]
pub use link::LinkManager;
#[cfg(feature = "prodj")]
pub use prodj::ProDjManager;
