// macOS: Syphon *linking* (framework search path + link-lib) is fully handled
// by the syphon-core crate, which reassembles its bundled Syphon.framework in
// its OUT_DIR — nothing here is needed to build. But `-rpath` link-args do NOT
// propagate across crates, so every leaf binary (and every lib whose own test
// binaries link Syphon) re-emits the runtime search paths: a locally available
// framework for `cargo run`/`cargo test`, /Library/Frameworks for system-wide
// installs, and bundle-relative rpaths for packaged .apps (release packaging
// copies Syphon.framework into <app>.app/Contents/Frameworks).
fn main() {
    #[cfg(target_os = "macos")]
    {
        // Dev-run rpath (optional): a local Syphon.framework, if one is around.
        if let Some(dir) = local_syphon_framework() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        // System-wide install (official Syphon installer).
        println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/Frameworks");

        // NDI rpath: always add if installed (for binaries built with `ndi`)
        let ndi_lib_paths = ["/usr/local/lib", "/Library/NDI SDK for Apple/lib/macOS"];
        for path in &ndi_lib_paths {
            if std::path::Path::new(path).exists() {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path);
            }
        }

        // AVFoundation (camera authorization)
        println!("cargo:rustc-link-lib=framework=AVFoundation");

        // Bundle-friendly rpaths
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");

        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-env-changed=SYPHON_FRAMEWORK_DIR");
    }
}

/// A Syphon.framework for dev runs, if available. Never required: without it,
/// dyld falls back to /Library/Frameworks or a bundled copy in the .app.
#[cfg(target_os = "macos")]
fn local_syphon_framework() -> Option<std::path::PathBuf> {
    // 1. Explicit override
    if let Ok(dir) = std::env::var("SYPHON_FRAMEWORK_DIR") {
        let p = std::path::PathBuf::from(dir);
        if p.join("Syphon.framework").exists() {
            return Some(p);
        }
    }

    // 2. syphon-rs checkout next to this repo
    //    (crate dirs are two levels below the repo root)
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest.ancestors().nth(3)?.join("syphon-rs/syphon-lib");
    if candidate.join("Syphon.framework").exists() {
        return Some(candidate);
    }

    None
}
