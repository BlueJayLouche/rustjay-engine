// The mixer depends on `rustjay-render`, which links `Syphon.framework` on
// macOS. The framework *link* propagates to downstream crates via
// `rustc-link-lib`, but the `-rpath` link-arg does NOT (see
// `rustjay-render/build.rs`). Without re-emitting the rpath here, this crate's
// own test binary links Syphon but can't locate it at load time and aborts with
// a dyld error. Every leaf binary in the workspace (engine + examples) re-emits
// the rpath for the same reason; a lib-with-tests is effectively a leaf too.
fn main() {
    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = find_syphon_framework() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.to_string_lossy());
        }
        // Bundle-friendly rpaths, matching rustjay-render/build.rs.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-env-changed=SYPHON_FRAMEWORK_DIR");
    }
}

#[cfg(target_os = "macos")]
fn find_syphon_framework() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("SYPHON_FRAMEWORK_DIR") {
        let p = std::path::PathBuf::from(dir);
        if p.join("Syphon.framework").exists() {
            return Some(p);
        }
    }
    let cargo_home = std::env::var("CARGO_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".cargo")));
    let cargo_home = cargo_home?;
    let checkouts = cargo_home.join("git/checkouts");
    let entries = std::fs::read_dir(&checkouts).ok()?;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("syphon-rs") {
            if let Ok(revs) = std::fs::read_dir(entry.path()) {
                for rev in revs.flatten() {
                    let candidate = rev.path().join("syphon-lib");
                    if candidate.join("Syphon.framework").exists() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}
