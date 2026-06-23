//! wasmtime-based plugin host.
//!
//! Loads `.wasm` modules compiled for `wasm32-unknown-unknown` and calls
//! lifecycle hooks exported by the guest.

use std::path::Path;
use wasmtime::{Caller, Engine, Instance, Linker, Module, Store};

/// User data attached to each plugin `Store`.
pub struct PluginState;

/// Error type for plugin operations.
#[derive(thiserror::Error, Debug)]
pub enum PluginError {
    #[error("wasmtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manages wasmtime compilation resources.
pub struct PluginHost {
    engine: Engine,
}

impl PluginHost {
    pub fn new() -> Result<Self, PluginError> {
        let engine = Engine::default();
        Ok(Self { engine })
    }

    /// Load a `.wasm` file and instantiate it with host imports.
    pub fn load(&self, path: &Path) -> Result<PluginInstance, PluginError> {
        let module = Module::from_file(&self.engine, path)?;
        let mut linker = Linker::new(&self.engine);

        // Host import: log a message from the plugin.
        // level: 0=info, 1=warn, 2=error
        linker.func_wrap(
            "env",
            "host_log",
            |mut caller: Caller<'_, PluginState>, level: i32, ptr: i32, len: i32| {
                if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let len = len.max(0) as usize;
                    let mut buf = vec![0u8; len];
                    if memory.read(&caller, ptr as usize, &mut buf).is_ok() {
                        if let Ok(s) = std::str::from_utf8(&buf) {
                            match level {
                                0 => log::info!("[plugin] {}", s),
                                1 => log::warn!("[plugin] {}", s),
                                2 => log::error!("[plugin] {}", s),
                                _ => log::info!("[plugin] {}", s),
                            }
                        }
                    }
                }
            },
        )?;

        let mut store = Store::new(&self.engine, PluginState);
        let instance = linker.instantiate(&mut store, &module)?;
        let memory = instance.get_memory(&mut store, "memory");

        Ok(PluginInstance {
            store,
            instance,
            memory,
        })
    }
}

/// A loaded plugin instance with callable lifecycle hooks.
pub struct PluginInstance {
    store: Store<PluginState>,
    instance: Instance,
    #[allow(dead_code)]
    memory: Option<wasmtime::Memory>,
}

impl PluginInstance {
    /// Call an optional exported function with no parameters.
    fn call_void(&mut self, name: &str) -> Result<(), PluginError> {
        if let Some(func) = self.instance.get_func(&mut self.store, name) {
            func.call(&mut self.store, &[], &mut [])?;
        }
        Ok(())
    }

    /// Call an optional exported function with one i32 parameter.
    fn call_i32(&mut self, name: &str, arg: i32) -> Result<(), PluginError> {
        if let Some(func) = self.instance.get_func(&mut self.store, name) {
            func.call(&mut self.store, &[arg.into()], &mut [])?;
        }
        Ok(())
    }

    pub fn on_load(&mut self) -> Result<(), PluginError> {
        self.call_void("qplayer_plugin_on_load")
    }

    pub fn on_unload(&mut self) -> Result<(), PluginError> {
        self.call_void("qplayer_plugin_on_unload")
    }

    pub fn on_go(&mut self, cue_qid: i32) -> Result<(), PluginError> {
        self.call_i32("qplayer_plugin_on_go", cue_qid)
    }

    pub fn on_save(&mut self) -> Result<(), PluginError> {
        self.call_void("qplayer_plugin_on_save")
    }

    pub fn on_slow_update(&mut self) -> Result<(), PluginError> {
        self.call_void("qplayer_plugin_on_slow_update")
    }
}
