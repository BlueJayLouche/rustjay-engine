fn main() {
    // The Blackmagic DeckLink SDK and its COM bindings are Windows-only. On any
    // other target, skip the C++ wrapper compile and the ole32/oleaut32 link so
    // the crate still builds (as a no-op) in a cross-platform `cargo build
    // --workspace`. The Rust side is `#[cfg(windows)]`-gated to match.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // `DeckLinkAPI.h` is Blackmagic's proprietary SDK header — we can't
    // redistribute it, so it's git-ignored and the user supplies their own copy.
    if !std::path::Path::new("src/DeckLinkAPI.h").exists() {
        panic!(
            "examples/decklink: src/DeckLinkAPI.h not found.\n\
             Copy DeckLinkAPI.h from your installed Blackmagic DeckLink SDK \
             (the `Win/include` directory) into `examples/decklink/src/`. It is \
             git-ignored because the SDK header cannot be redistributed. See the README."
        );
    }

    cc::Build::new()
        .cpp(true)
        .file("src/decklink_sdk_wrapper.cpp")
        .compile("decklink_wrapper");

    println!("cargo:rustc-link-lib=ole32");
    println!("cargo:rustc-link-lib=oleaut32");
    println!("cargo:rerun-if-changed=src/DeckLinkAPI.h");
    println!("cargo:rerun-if-changed=src/decklink_sdk_wrapper.cpp");
}
