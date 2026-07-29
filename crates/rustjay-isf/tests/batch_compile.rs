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

/// 00-vangogh.fs has `//` comments inside its header JSON (serde_json is strict).
/// Strip them before `isf::parse` — the leniency belongs at the parse boundary.
fn strip_json_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut prev = '\0';
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '"' && prev != '\\' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            for c2 in chars.by_ref() {
                if c2 == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else {
            out.push(c);
        }
        prev = c;
    }
    out
}

/// Parse + compile one shader file. Err strings are categorized loosely for reporting.
fn compile_one(path: &PathBuf) -> Result<rustjay_isf::Transpiled, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let src = strip_json_comments(&src);
    let isf = isf::parse(&src).map_err(|e| format!("header-parse: {e}"))?;
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
