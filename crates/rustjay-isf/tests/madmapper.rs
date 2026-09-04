//! MadMapper materials are ISF with a different entry point, host-driven
//! `GENERATORS`, and a looser header. They must transpile like any other
//! shader.

fn material(header: &str, body: &str) -> String {
    format!("/*{{\n{header}\n}}*/\n{body}")
}

#[test]
fn a_material_gets_a_main_that_calls_it() {
    let src = material(
        r#""INPUTS": [ { "NAME": "mat_amount", "TYPE": "float", "MAX": 2., "DEFAULT": .5 } ]"#,
        "vec4 materialColorForPixel(vec2 texCoord) {\n\
         return vec4(texCoord * mat_amount, 0.0, 1.0);\n}",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert!(out.wgsl.contains("materialColorForPixel"), "{}", out.wgsl);
}

#[test]
fn a_time_base_generator_becomes_a_value_that_moves() {
    let src = material(
        r#""INPUTS": [ { "NAME": "mat_speed", "TYPE": "float", "DEFAULT": 1.0 },
                       { "NAME": "mat_back", "TYPE": "bool", "DEFAULT": false } ],
           "GENERATORS": [ { "NAME": "mat_pos", "TYPE": "time_base",
                             "PARAMS": { "speed": "mat_speed", "reverse": "mat_back" } } ]"#,
        "vec4 materialColorForPixel(vec2 texCoord) {\n\
         return vec4(fract(mat_pos), texCoord, 1.0);\n}",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    // The generator is not an INPUT, so it only resolves if we declared it.
    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    // It must be driven by TIME, not left at zero.
    assert!(out.wgsl.contains("mat_pos"), "{}", out.wgsl);
    assert!(!out.manifest.input_fields.iter().any(|f| f.name == "mat_pos"));
}

#[test]
fn a_generator_of_an_unsupported_kind_still_compiles() {
    let src = material(
        r#""GENERATORS": [ { "NAME": "mat_x", "TYPE": "no_such_generator" } ]"#,
        "vec4 materialColorForPixel(vec2 uv) { return vec4(mat_x); }",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    rustjay_isf::compile::compile(&isf, &src).expect("compiles");
}
