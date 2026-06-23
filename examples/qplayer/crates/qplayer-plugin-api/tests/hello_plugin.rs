//! Integration test: load the hello-plugin `.wasm` and exercise lifecycle hooks.

use std::path::PathBuf;

fn wasm_path() -> PathBuf {
    // Workspace target directory structure:
    // target/wasm32-unknown-unknown/debug/hello_plugin.wasm
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates
    path.pop(); // workspace root
    path.push("target");
    path.push("wasm32-unknown-unknown");
    path.push("debug");
    path.push("hello_plugin.wasm");
    path
}

#[test]
fn test_load_hello_plugin() {
    let path = wasm_path();
    assert!(
        path.exists(),
        "hello_plugin.wasm not found at {:?}. Build it first:\n  cargo build --target wasm32-unknown-unknown -p hello-plugin",
        path
    );

    let host = qplayer_plugin_api::PluginHost::new().expect("create host");
    let mut plugin = host.load(&path).expect("load plugin");

    // All hooks should succeed (or be no-ops if not exported)
    plugin.on_load().expect("on_load");
    plugin.on_go(1).expect("on_go");
    plugin.on_save().expect("on_save");
    plugin.on_slow_update().expect("on_slow_update");
    plugin.on_unload().expect("on_unload");
}

#[test]
fn test_plugin_crash_isolation() {
    // A missing function is silently ignored; a trap from the plugin
    // would be caught as an Err from wasmtime. Our hello-plugin does
    // not trap, so this test just verifies the host doesn't panic.
    let path = wasm_path();
    if !path.exists() {
        return; // Skip if wasm not built
    }

    let host = qplayer_plugin_api::PluginHost::new().expect("create host");
    let mut plugin = host.load(&path).expect("load plugin");

    // Repeated calls should be fine
    for _ in 0..10 {
        plugin.on_slow_update().expect("on_slow_update");
    }
}
