//! Shaders written for MadMapper `#include` its GLSL libraries. Those are
//! vendored, so the include resolves instead of taking the compile down.

fn shader(body: &str) -> String {
    format!(
        "/*{{ \"INPUTS\": [] }}*/\n{body}\nvoid main() {{\n\
         gl_FragColor = vec4(hsv2rgb(vec3(TIME, 1.0, saturate(luma(vec3(0.5))))), 1.0);\n}}"
    )
}

#[test]
fn a_madcommon_include_resolves() {
    let src = shader("#include \"MadCommon.glsl\"");
    let isf = rustjay_isf::header::parse(&src).expect("header");

    rustjay_isf::generate_wgsl(&isf, &src).expect("transpiles");
}

// MadMapper writes the include with a directory in front of it.
#[test]
fn a_path_prefixed_include_resolves_by_its_file_name() {
    let src = shader("#include \"Libraries/MadCommon.glsl\"");
    let isf = rustjay_isf::header::parse(&src).expect("header");

    rustjay_isf::generate_wgsl(&isf, &src).expect("transpiles");
}

#[test]
fn an_unbundled_include_says_what_is_on_offer() {
    let src = shader("#include \"MadeUp.glsl\"");
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let Err(err) = rustjay_isf::generate_wgsl(&isf, &src) else {
        panic!("an unbundled include should not compile");
    };
    assert!(err.contains("MadCommon.glsl"), "{err}");
}
