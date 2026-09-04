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

    // It gets a field of its own, which the host drives per frame.
    assert!(out.manifest.input_fields.iter().any(|f| f.name == "mat_pos"));
    assert_eq!(out.manifest.generators.len(), 1);
    assert_eq!(out.manifest.generators[0].ty, "time_base");
    assert_eq!(out.manifest.generators[0].params["speed"], "mat_speed");
}

// A generator sharing a name with an input would declare the field twice.
#[test]
fn a_generator_shadowed_by_an_input_does_not_get_a_second_field() {
    let src = material(
        r#""INPUTS": [ { "NAME": "mat_x", "TYPE": "float", "DEFAULT": 1.0 } ],
           "GENERATORS": [ { "NAME": "mat_x", "TYPE": "time_base" } ]"#,
        "vec4 materialColorForPixel(vec2 uv) { return vec4(mat_x); }",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert_eq!(
        out.manifest.input_fields.iter().filter(|f| f.name == "mat_x").count(),
        1
    );
    assert!(out.manifest.generators.is_empty());
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

// The prelude already rewrites a whole `texture2D(t, uv)` call, so wrapping its
// first argument as well would construct a sampler from a sampler.
#[test]
fn a_legacy_texture2d_call_is_not_wrapped_twice() {
    let src = material(
        r#""INPUTS": [ { "NAME": "inputImage", "TYPE": "image" } ]"#,
        "vec4 materialColorForPixel(vec2 uv) { return texture2D(inputImage, uv); }",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    rustjay_isf::compile::compile(&isf, &src).expect("compiles");
}

// A shader that samples an input the GL way, rather than through IMG_*.
#[test]
fn a_directly_sampled_input_gets_the_sampler() {
    let src = material(
        r#""INPUTS": [ { "NAME": "spectrum", "TYPE": "audioFFT", "SIZE": 16 } ]"#,
        "vec4 materialColorForPixel(vec2 uv) { return texture(spectrum, vec2(uv.x, 0.5)); }",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    rustjay_isf::compile::compile(&isf, &src).expect("compiles");
}
