//! Hard gate: every bundled shader must compile through the new ISF pipeline
//! (prelude-GLSL → shaderc → naga → WGSL), except a short known-broken list.
//!
//! An extended corpus test (gated on `ISF_CORPUS_DIR`) runs a full stock-ISF
//! corpus informationally and reports pass rate + failure categories.

use std::path::PathBuf;

/// Bundled shaders that are genuinely broken (not pipeline limitations):
/// - StateOfPlanet*.fs: `vec3(vec2, float, float)` constructor — invalid GLSL in any compiler.
/// - gpt-cosmic-wave.fs: file starts with a markdown ```glsl fence, no ISF header.
const KNOWN_BROKEN: [&str; 3] = [
    "StateOfPlanetFullscreen.fs",
    "StateOfPlanetInfinite.fs",
    "gpt-cosmic-wave.fs",
];

fn shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders")
}


/// Parse + compile one shader file. Err strings are categorized loosely for reporting.
fn compile_one(path: &PathBuf) -> Result<rustjay_isf::Transpiled, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let isf = rustjay_isf::header::parse(&src).map_err(|e| format!("header-parse: {e}"))?;
    rustjay_isf::generate_wgsl(&isf, &src)
}

#[test]
fn bundled_shaders_compile() {
    let mut failures = Vec::new();
    let mut ok = 0;
    let mut entries: Vec<_> = std::fs::read_dir(shaders_dir())
        .expect("shaders dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("fs"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "no bundled shaders found");

    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match compile_one(path) {
            Ok(t) => {
                // generate_wgsl already re-parses + re-validates the WGSL with naga;
                // an Ok result is well-formed by construction.
                assert!(!t.wgsl.is_empty(), "{name}: empty WGSL");
                assert!(!t.manifest.frag_entry.is_empty(), "{name}: no frag entry");
                ok += 1;
            }
            Err(e) => {
                if KNOWN_BROKEN.contains(&name.as_str()) {
                    continue;
                }
                failures.push(format!("{name}: {e}"));
            }
        }
    }
    eprintln!("bundled shaders: {ok} ok, {} failed", failures.len());
    assert!(failures.is_empty(), "compile failures:\n{}", failures.join("\n"));
}

/// Informational extended-corpus run: never fails. Run with:
/// `ISF_CORPUS_DIR=/path/to/corpus cargo test -p rustjay-isf --test batch_compile corpus -- --nocapture`
#[test]
fn corpus_compile_rate() {
    let Ok(dir) = std::env::var("ISF_CORPUS_DIR") else {
        eprintln!("ISF_CORPUS_DIR not set; skipping extended corpus test");
        return;
    };
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("corpus dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("fs"))
        .collect();
    entries.sort();

    let mut ok = 0usize;
    let mut categories: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match compile_one(path) {
            Ok(_) => ok += 1,
            Err(e) => {
                let cat = if e.starts_with("header-parse") || e.starts_with("read") {
                    "header-parse"
                } else if e.contains("overloaded functions must have the same parameter precision") {
                    "builtin-fn-redefine"
                } else if e.contains("NotIOShareableType") {
                    "naga-array-varying"
                } else if e.contains("ExpressionAlreadyInScope") {
                    "naga-overload-bug"
                } else if e.starts_with("naga") {
                    "naga-other"
                } else if e.starts_with("wgsl") {
                    "wgsl"
                } else {
                    "shaderc"
                };
                categories.entry(cat.to_string()).or_default().push(name);
            }
        }
    }
    let total = entries.len();
    eprintln!("\n=== corpus {} ===", dir);
    eprintln!("{ok}/{total} OK ({:.1}%)", ok as f64 / total.max(1) as f64 * 100.0);
    for (cat, names) in &categories {
        eprintln!("  [{cat}] ({}): {}", names.len(), names.join(", "));
    }
}

/// The vertex stage delivers `isf_FragNormCoord` Y-flipped into ISF's
/// bottom-left convention, and the `IMG_*` sampling rewrite flips it back. A
/// shader that samples directly never goes through that rewrite, so flipping
/// for it inverts the output once per pass — which is what made an odd-length
/// FX chain render upside down.
///
/// None of the bundled shaders use the macros; they declare their own bindings
/// and call `texture(sampler2D(...), uv)`. So every one of them must opt out.
#[test]
fn baked_dialect_shaders_do_not_get_the_y_flip() {
    let mut checked = 0;
    let mut wrongly_flipped = Vec::new();

    for entry in std::fs::read_dir(shaders_dir()).expect("shaders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("fs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if KNOWN_BROKEN.contains(&name.as_str()) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(isf) = rustjay_isf::header::parse(&src) else {
            continue;
        };
        let Ok(out) = rustjay_isf::compile::compile(&isf, &src) else {
            continue;
        };

        let uses_macros = src.contains("IMG_THIS_PIXEL")
            || src.contains("IMG_NORM_PIXEL")
            || src.contains("IMG_PIXEL");
        checked += 1;
        if out.manifest.flip_frag_norm_coord != uses_macros {
            wrongly_flipped.push(name);
        }
    }

    assert!(checked > 50, "expected the bundled corpus, compiled {checked}");
    assert!(
        wrongly_flipped.is_empty(),
        "these shaders have the wrong Y-flip setting: {wrongly_flipped:?}"
    );
}
