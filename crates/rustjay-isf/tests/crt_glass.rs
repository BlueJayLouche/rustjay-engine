//! The bundled CRT-Glass filter (used by the shaderglass example) must parse
//! and transpile to WGSL offline — otherwise it only fails at GPU init.

use std::path::PathBuf;

#[test]
fn crt_glass_transpiles() {
    transpiles("CRT-Glass.fs");
}

#[test]
fn shaderbeam_transpiles() {
    transpiles("ShaderBeam.fs");
}

fn transpiles(name: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("shaders")
        .join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {name}"));
    let isf = isf::parse(&src).expect("ISF header parses");
    rustjay_isf::generate_wgsl(&isf, &src).unwrap_or_else(|e| panic!("{name} transpile: {e}"));
}
