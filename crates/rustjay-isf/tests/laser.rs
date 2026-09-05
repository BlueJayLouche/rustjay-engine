//! MadMapper laser materials draw 2D paths, not pixels: one invocation per
//! sample point, writing into a `POINT_COUNT` x 3 target.

fn laser(header: &str, body: &str) -> String {
    format!("/*{{\n{header}\n}}*/\n{body}")
}

const CIRCLE: &str = "void laserMaterialFunc(int pointNumber, int pointCount, out vec2 pos, \
     out vec4 color, out int shapeNumber, out vec4 userData) {\n\
     float t = float(pointNumber) / float(pointCount - 1);\n\
     pos = vec2(cos(t * 6.283), sin(t * 6.283));\n\
     color = vec4(1.0);\n\
     shapeNumber = 0;\n}";

#[test]
fn a_laser_material_compiles_and_is_marked_as_one() {
    let src = laser(r#""INPUTS": [], "RENDER_SETTINGS": { "POINT_COUNT": 500 }"#, CIRCLE);
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    let settings = out.manifest.laser.expect("recognised as a laser material");
    assert_eq!(settings.point_count, Some(500));
}

// The same thing without the feedback channel — 12 of MadMapper's 109 use it.
#[test]
fn a_vector_material_bridges_without_user_data() {
    let src = laser(
        r#""INPUTS": []"#,
        "void vectorMaterialFunc(int pointNumber, int pointCount, out vec2 pos, \
         out vec4 color, out int shapeNumber) {\n\
         pos = vec2(float(pointNumber) / float(pointCount));\n\
         color = vec4(1.0);\n shapeNumber = 0;\n}",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert!(out.manifest.laser.is_some());
}

// A material with no RENDER_SETTINGS is still a laser material; the host picks
// the point count.
#[test]
fn a_laser_material_without_render_settings_still_compiles() {
    let src = laser(r#""INPUTS": []"#, CIRCLE);
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert_eq!(out.manifest.laser.expect("laser").point_count, None);
}

// The previous frame's output is bound by the host, not declared in the header,
// so it only becomes a texture binding when the shader actually reads it.
#[test]
fn reading_the_last_frame_declares_it_as_a_texture() {
    let with = laser(
        r#""INPUTS": [], "RENDER_SETTINGS": { "POINT_COUNT": 64 }"#,
        "void laserMaterialFunc(int pointNumber, int pointCount, out vec2 pos, \
         out vec4 color, out int shapeNumber, out vec4 userData) {\n\
         pos = texelFetch(mm_LastFrameData, ivec2(pointNumber, 0), 0).rg * 0.9;\n\
         color = vec4(1.0);\n shapeNumber = 0;\n}",
    );
    let isf = rustjay_isf::header::parse(&with).expect("header");
    let out = rustjay_isf::compile::compile(&isf, &with).expect("compiles");
    assert!(out.manifest.textures.iter().any(|t| t.name == "mm_LastFrameData"));

    let without = laser(r#""INPUTS": []"#, CIRCLE);
    let isf = rustjay_isf::header::parse(&without).expect("header");
    let out = rustjay_isf::compile::compile(&isf, &without).expect("compiles");
    assert!(out.manifest.textures.is_empty());
}

// Laser materials share the rest of the dialect: 95 of the 109 use generators.
#[test]
fn a_laser_material_gets_its_generators() {
    let src = laser(
        r#""INPUTS": [ { "NAME": "mat_speed", "TYPE": "float", "DEFAULT": 1. } ],
           "GENERATORS": [ { "NAME": "mat_t", "TYPE": "time_base",
                             "PARAMS": { "speed": "mat_speed" } } ]"#,
        "void laserMaterialFunc(int pointNumber, int pointCount, out vec2 pos, \
         out vec4 color, out int shapeNumber, out vec4 userData) {\n\
         pos = vec2(fract(mat_t));\n color = vec4(1.0);\n shapeNumber = 0;\n}",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert_eq!(out.manifest.generators.len(), 1);
}

// A video material must not be mistaken for a laser one.
#[test]
fn a_video_material_is_not_a_laser_material() {
    let src = laser(
        r#""INPUTS": []"#,
        "vec4 materialColorForPixel(vec2 uv) { return vec4(uv, 0.0, 1.0); }",
    );
    let isf = rustjay_isf::header::parse(&src).expect("header");

    let out = rustjay_isf::compile::compile(&isf, &src).expect("compiles");

    assert!(out.manifest.laser.is_none());
}
