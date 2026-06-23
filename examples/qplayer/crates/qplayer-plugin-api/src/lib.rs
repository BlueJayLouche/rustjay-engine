//! QPlayer Plugin API — WASM plugin host interface.
//!
//! Plugins compile to `.wasm` modules (target `wasm32-unknown-unknown`) and are
//! executed in a `wasmtime` sandbox.
//!
//! # Guest ABI
//!
//! The plugin may export any of the following functions:
//!
//! | Export | Signature | Called when |
//! |--------|-----------|-------------|
//! | `qplayer_plugin_on_load` | `fn()` | After successful load |
//! | `qplayer_plugin_on_unload` | `fn()` | Before the host exits |
//! | `qplayer_plugin_on_go` | `fn(qid: i32)` | A cue is started |
//! | `qplayer_plugin_on_save` | `fn()` | The show file is saved |
//! | `qplayer_plugin_on_slow_update` | `fn()` | Every 250 ms |
//!
//! # Host imports
//!
//! The host provides one import module (`env`) with one function:
//!
//! `host_log(level: i32, ptr: i32, len: i32)` — logs a UTF-8 string from the
//! plugin's linear memory. `level`: 0=info, 1=warn, 2=error.

pub mod host;

pub use host::{PluginError, PluginHost, PluginInstance, PluginState};
