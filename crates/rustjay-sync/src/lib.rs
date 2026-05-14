//! rustjay-sync — Ableton Link and ProDJ Link tempo sync.
//!
//! This crate provides optional integrations with industry-standard sync
//! protocols. Each protocol is gated by a Cargo feature so apps only pay
//! compile-time cost for what they use.
//!
//! | Feature | Protocol | License of dep |
//! |---------|----------|----------------|
//! | `link`  | Ableton Link | GPL-2.0+ |
//! | `prodj` | Pioneer ProDJ Link | MIT |
//!
//! **Important:** Enabling the `link` feature links against Ableton Link,
//! which is GPL-2.0+. The resulting binary is subject to GPL terms.

#![warn(missing_docs)]

#[cfg(feature = "link")]
pub mod link;
#[cfg(feature = "prodj")]
pub mod prodj;

#[cfg(feature = "link")]
pub use link::LinkManager;
#[cfg(feature = "prodj")]
pub use prodj::ProDjManager;
