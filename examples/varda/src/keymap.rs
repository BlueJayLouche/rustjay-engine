//! Keymap — keyboard shortcut bindings.
//!
//! Persisted as `.varda/keymap.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One keyboard binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    pub modifiers: Vec<String>,
    pub action: String,
}

/// Simple keymap layer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Keymap {
    pub bindings: HashMap<String, KeyBinding>,
}

impl Keymap {
    pub fn default_bindings() -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(
            "save".to_string(),
            KeyBinding {
                key: "S".to_string(),
                modifiers: vec!["Command".to_string()],
                action: "workspace.save".to_string(),
            },
        );
        Self { bindings }
    }
}
