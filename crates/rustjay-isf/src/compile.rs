//! ISF compile core: prelude-GLSL generation → shaderc (Vulkan 1.2) → naga spv-in → WGSL.
//!
//! Replaces the hand-rolled textual transpiler for the GLSL→WGSL conversion itself.
//! Validated against ~380 stock ISF shaders (see `tests/batch_compile.rs`): ~93-95%
//! of the corpus compiles cleanly.
//!
//! ## Generated GPU ABI (single bind group set)
//!
//! - binding 0: `uniform IsfData { PASSINDEX, RENDERSIZE, TIME, TIMEDELTA, DATE, FRAMEINDEX }`
//! - binding 1: `uniform IsfInputs { ... }` — omitted when the shader declares no inputs
//! - binding 2: `sampler img_sampler` — only when any texture binding exists
//! - binding 3+: `texture2D` per image/audio input, then PASSES targets, then IMPORTED names
//!
//! ## Y-flip convention
//!
//! ISF is bottom-left origin; our textures are top-left origin. The runtime vertex
//! shader delivers `isf_FragNormCoord` Y-flipped to ISF's convention, and all texture
//! sampling generated here flips the coordinate back, so `IMG_THIS_PIXEL(inputImage)`
//! hits the same physical pixel the CPU-side caller expects.
//!
//! ponytail: the body rewrites below are line/char scanners, not a GLSL parser.
//! Ceiling: pathological formatting (multi-line declarations, comments masquerading as
//! code) can fool them; the corpus shows this is rare. Upgrade path = a real GLSL parser.

use isf::{InputType, Isf};

/// GPU manifest: everything the runtime needs to build the bind group layout and
/// pack the inputs uniform buffer.
#[derive(Clone, Debug)]
pub struct IsfManifest {
    /// std140 field map of the `IsfInputs` block (declaration order).
    pub input_fields: Vec<InputField>,
    /// std140 size of the `IsfInputs` block; 0 when the block is omitted.
    pub inputs_block_size: usize,
    /// Texture bindings in declaration order (bindings start at 3).
    pub textures: Vec<TextureBinding>,
    /// True when any texture binding exists (sampler at binding 2 present).
    pub has_sampler: bool,
    /// Fragment entry point name in the emitted WGSL (from the naga module).
    pub frag_entry: String,
}

/// One std140 field of the `IsfInputs` block.
#[derive(Clone, Debug)]
pub struct InputField {
    pub name: String,
    pub offset: usize,
    pub ty: FieldTy,
}

/// Scalar/vector field types we declare in the inputs block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldTy {
    F32,
    I32,
    Bool,
    Vec2,
    Vec3,
    Vec4,
}

/// A texture binding (separate `texture2D`; sampled through the shared `img_sampler`).
#[derive(Clone, Debug)]
pub struct TextureBinding {
    pub name: String,
    pub binding: u32,
}

/// Output of a successful compile.
pub struct CompileOutput {
    pub wgsl: String,
    pub manifest: IsfManifest,
}

/// Compile ISF GLSL source to WGSL.
///
/// `isf` is the parsed header metadata; `glsl_src` is the full shader file
/// (header comment included — it is stripped here).
pub fn compile(isf: &Isf, glsl_src: &str) -> Result<CompileOutput, String> {
    let body = strip_header(glsl_src)?;
    let merged = build_glsl(isf, &body);

    // GLSL → SPIR-V (Vulkan 1.2, fragment, entry "main").
    let compiler =
        shaderc::Compiler::new().map_err(|e| format!("shaderc: failed to create compiler: {e}"))?;
    let mut opts = shaderc::CompileOptions::new()
        .map_err(|e| format!("shaderc: failed to create options: {e}"))?;
    opts.set_source_language(shaderc::SourceLanguage::GLSL);
    opts.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_2 as u32,
    );
    opts.set_optimization_level(shaderc::OptimizationLevel::Zero); // keep names for error msgs
    let artifact = compiler
        .compile_into_spirv(&merged.glsl, shaderc::ShaderKind::Fragment, "isf.fs", "main", Some(&opts))
        .map_err(|e| trim_err(&e.to_string()))?;

    // SPIR-V → naga module → validate → WGSL.
    let words = artifact.as_binary();
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let module = naga::front::spv::parse_u8_slice(&bytes, &naga::front::spv::Options::default())
        .map_err(|e| format!("naga spv-in: {}", trim_err(&format!("{e:?}"))))?;
    let frag_entry = module
        .entry_points
        .iter()
        .find(|ep| ep.stage == naga::ShaderStage::Fragment)
        .map(|ep| ep.name.clone())
        .unwrap_or_else(|| "main".to_string());
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("naga validate: {}", trim_err(&format!("{e:?}"))))?;
    let wgsl = naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| format!("naga wgsl-out: {e:?}"))?;

    // Prove the WGSL is well-formed for wgpu (re-parse + re-validate).
    let reparsed = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|e| format!("wgsl re-parse: {}", trim_err(&e.to_string())))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&reparsed)
    .map_err(|e| format!("wgsl re-validate: {e:?}"))?;

    let (input_fields, inputs_block_size) = layout_std140(&merged.members);
    let manifest = IsfManifest {
        input_fields,
        inputs_block_size,
        has_sampler: !merged.textures.is_empty(),
        textures: merged.textures,
        frag_entry,
    };
    Ok(CompileOutput { wgsl, manifest })
}

/// Keep error strings to a useful excerpt.
fn trim_err(e: &str) -> String {
    e.chars().take(500).collect()
}

// ---------------------------------------------------------------------------
// GLSL generation
// ---------------------------------------------------------------------------

/// Compat defines: legacy WebGL-era sampler calls route through the shared sampler.
/// glslang accepts function-like macros named after reserved words (PoC-verified);
/// `#define GL_ES` on the other hand is rejected, which is why GL_ES-guarded code
/// is handled textually below.
const DEFINES: &str = "\
#define texture2D(t, c) texture(sampler2D(t, img_sampler), c)
#define textureCube(t, c) texture(samplerCube(t, img_sampler), c)
#define attribute in
";

/// Builtin names provided by the prelude; body redeclarations of these are dropped.
const BUILTINS: [&str; 7] = [
    "PASSINDEX",
    "RENDERSIZE",
    "TIME",
    "TIMEDELTA",
    "DATE",
    "FRAMEINDEX",
    "isf_FragNormCoord",
];

/// GLSL builtins that stock shaders sometimes redefine inside `#ifndef GL_ES` guards.
const REDEFINED_BUILTINS: [&str; 17] = [
    "distance", "round", "trunc", "mod", "fract", "min", "max", "clamp", "mix", "step",
    "smoothstep", "length", "normalize", "dot", "cross", "reflect", "refract",
];

/// One member of the generated `IsfInputs` block.
struct MemberDecl {
    name: String,
    /// GLSL declaration text, e.g. "float" or "float[4]" for re-homed array uniforms.
    glsl_decl: String,
    /// Type used for std140 layout. ponytail: re-homed uniforms with exotic types
    /// (mat*, arrays) fall back to F32 — layout is correct for every type observed
    /// in the corpus (float/int/bool/vec2/vec4); exotic re-homed uniforms may get
    /// wrong offsets. Upgrade path = full std140 type model.
    fty: FieldTy,
}

struct Merged {
    glsl: String,
    members: Vec<MemberDecl>,
    textures: Vec<TextureBinding>,
}

fn build_glsl(isf: &Isf, raw_body: &str) -> Merged {
    // 1. drop existing #version lines
    let mut body: String = raw_body
        .lines()
        .filter(|l| !l.trim_start().starts_with("#version"))
        .collect::<Vec<_>>()
        .join("\n");

    // Names our prelude provides (builtins, header inputs, per-image aux fields).
    let provided: Vec<String> = BUILTINS
        .iter()
        .map(|s| s.to_string())
        .chain(isf.inputs.iter().flat_map(|i| {
            [
                i.name.clone(),
                format!("_{}_imgRect", i.name),
                format!("_{}_imgSize", i.name),
                format!("_{}_flip", i.name),
            ]
        }))
        .collect();
    let provided: Vec<&str> = provided.iter().map(|s| s.as_str()).collect();
    // Texture names we will declare (image/audio inputs, pass targets, imported).
    let mut texture_names: Vec<String> = isf
        .inputs
        .iter()
        .filter(|i| {
            matches!(
                i.ty,
                InputType::Image | InputType::Audio(_) | InputType::AudioFft(_)
            )
        })
        .map(|i| i.name.clone())
        .chain(isf.passes.iter().filter_map(|p| p.target.clone()))
        .chain(isf.imported.keys().cloned())
        .collect();

    // 1.5 vjarda dialect: strip baked varda preludes (blocks/layout decls/aliases).
    let mut baked_members: Vec<MemberDecl> = Vec::new();
    let mut baked_textures: Vec<String> = Vec::new();
    let (b, baked_needs_out, bool_as_float) = strip_baked_prelude(
        &body,
        &provided,
        &texture_names,
        &mut baked_members,
        &mut baked_textures,
    );
    body = b;
    texture_names.extend(baked_textures.iter().cloned());

    // 2. excise `#ifndef GL_ES`-guarded redefinitions of GLSL builtins (we compile as
    //    desktop GL; the guarded fallback implementations only break glslang 450).
    body = excise_gles_builtin_redefs(&body);

    // 3. legacy varying name used by older ISF hosts
    body = body.replace("vv_FragNormCoord", "isf_FragNormCoord");

    // 4. gl_FragColor → declared out
    //    Shadertoy-style shaders with only `mainImage` (no `main`) get a bridge that
    //    also writes to the declared out.
    let has_main_image = body.contains("void mainImage") && find_main_def(&body).is_none();
    let needs_fragcolor_out = (body.contains("gl_FragColor")
        && !body.contains("out vec4 FragColor")
        && !body.contains("out vec4 gl_FragColor"))
        || has_main_image
        || baked_needs_out;
    if body.contains("gl_FragColor") {
        body = body.replace("gl_FragColor", "FragColor");
    }

    // 5. uniform dedup (always): bare non-block uniforms are illegal in Vulkan GLSL.
    //    Body redeclarations of prelude-provided names are dropped; unknown uniforms
    //    are re-homed into IsfInputs.
    let mut extra_members: Vec<MemberDecl> = Vec::new();
    body = dedup_uniforms(&body, &provided, &mut extra_members);
    extra_members.extend(baked_members);

    // 6. explicit locations for user-declared global in/out (incl. `varying`).
    //    isf_FragNormCoord takes in-location 0; FragColor takes out-location 0.
    body = assign_io_locations(&body, needs_fragcolor_out);

    // 7. inline IMG_* sampling helpers (Y-flip aware).
    body = inline_img_calls(&body);

    // 8. gl_FragCoord → flipped global via wrapper main; bare mainImage → bridge.
    let uses_fragcoord = body.contains("gl_FragCoord") || has_main_image;
    let body = if body.contains("gl_FragCoord") {
        let b = body.replace("gl_FragCoord", "isf_FragCoord");
        let b = rename_main(&b);
        format!(
            "{b}\nvoid main() {{\n    isf_FragCoord = vec2(gl_FragCoord.x, RENDERSIZE.y - gl_FragCoord.y);\n    isf_user_main();\n}}\n"
        )
    } else if has_main_image {
        format!(
            "{body}\nvoid main() {{\n    isf_FragCoord = vec2(gl_FragCoord.x, RENDERSIZE.y - gl_FragCoord.y);\n    mainImage(FragColor, isf_FragCoord);\n}}\n"
        )
    } else {
        body
    };

    // ---- prelude ----
    let mut p = String::from("#version 450\n");
    p.push_str(DEFINES);
    if needs_fragcolor_out {
        p.push_str("layout(location = 0) out vec4 FragColor;\n");
    }
    p.push_str("layout(location = 0) in vec2 isf_FragNormCoord;\n");
    if uses_fragcoord {
        p.push_str("vec2 isf_FragCoord;\n");
    }
    p.push_str(
        "layout(set = 0, binding = 0) uniform IsfData {\n    int PASSINDEX;\n    vec2 RENDERSIZE;\n    float TIME;\n    float TIMEDELTA;\n    vec4 DATE;\n    int FRAMEINDEX;\n};\n",
    );

    // inputs block members (in declaration order) + aux fields per image input
    let mut members: Vec<MemberDecl> = Vec::new();
    for input in &isf.inputs {
        let member = match &input.ty {
            InputType::Float(_) => Some(FieldTy::F32),
            InputType::Long(_) => Some(FieldTy::I32),
            InputType::Bool(_) | InputType::Event => {
                // vjarda dialect: baked blocks declare bools as float and bodies
                // compare against floats
                Some(if bool_as_float {
                    FieldTy::F32
                } else {
                    FieldTy::Bool
                })
            }
            InputType::Point2d(_) => Some(FieldTy::Vec2),
            InputType::Color(_) => Some(FieldTy::Vec4),
            InputType::Image | InputType::Audio(_) | InputType::AudioFft(_) => {
                // ISF per-image aux uniforms: _<name>_imgRect / _imgSize / _flip
                for (suffix, fty) in [
                    ("_imgRect", FieldTy::Vec4),
                    ("_imgSize", FieldTy::Vec2),
                    ("_flip", FieldTy::Bool),
                ] {
                    members.push(MemberDecl {
                        name: format!("_{}{suffix}", input.name),
                        glsl_decl: glsl_ty(fty).to_string(),
                        fty,
                    });
                }
                None
            }
        };
        if let Some(fty) = member {
            members.push(MemberDecl {
                name: input.name.clone(),
                glsl_decl: glsl_ty(fty).to_string(),
                fty,
            });
        }
    }
    members.extend(extra_members);

    // texture bindings: image/audio inputs, pass targets, imported, baked extras
    let textures: Vec<TextureBinding> = texture_names
        .iter()
        .enumerate()
        .map(|(i, name)| TextureBinding {
            name: name.clone(),
            binding: 3 + i as u32,
        })
        .collect();

    if !members.is_empty() {
        p.push_str("layout(set = 0, binding = 1) uniform IsfInputs {\n");
        for m in &members {
            p.push_str(&format!("    {} {};\n", m.glsl_decl, m.name));
        }
        p.push_str("};\n");
    }
    if !textures.is_empty() {
        p.push_str("layout(set = 0, binding = 2) uniform sampler img_sampler;\n");
        for t in &textures {
            p.push_str(&format!(
                "layout(set = 0, binding = {}) uniform texture2D {};\n",
                t.binding, t.name
            ));
        }
    }

    p.push_str(&body);
    Merged {
        glsl: p,
        members,
        textures,
    }
}

fn glsl_ty(fty: FieldTy) -> &'static str {
    match fty {
        FieldTy::F32 => "float",
        FieldTy::I32 => "int",
        FieldTy::Bool => "bool",
        FieldTy::Vec2 => "vec2",
        FieldTy::Vec3 => "vec3",
        FieldTy::Vec4 => "vec4",
    }
}

/// std140 offsets: scalars align 4, vec2 align 8, vec3/vec4 align 16; block size
/// rounded up to 16.
fn layout_std140(members: &[MemberDecl]) -> (Vec<InputField>, usize) {
    let mut offset = 0usize;
    let mut fields = Vec::with_capacity(members.len());
    for m in members {
        let (align, size) = match m.fty {
            FieldTy::F32 | FieldTy::I32 | FieldTy::Bool => (4, 4),
            FieldTy::Vec2 => (8, 8),
            FieldTy::Vec3 => (16, 12),
            FieldTy::Vec4 => (16, 16),
        };
        offset = offset.next_multiple_of(align);
        fields.push(InputField {
            name: m.name.clone(),
            offset,
            ty: m.fty,
        });
        offset += size;
    }
    let block_size = if members.is_empty() {
        0
    } else {
        offset.next_multiple_of(16)
    };
    (fields, block_size)
}

// ---------------------------------------------------------------------------
// Body rewrite passes (line/char scanners — see module docs)
// ---------------------------------------------------------------------------

/// Strip a baked varda-style prelude from the body (the 115 shaders bundled with
/// examples/vjarda were generated with declarations baked into the `.fs` file:
/// `ISFUniforms`/`UserParams`/`*Params` blocks, layout-qualified in/out, sampler and
/// texture decls). Triggered only when the body mentions `ISFUniforms` — stock ISF
/// never does. Members not provided by our own prelude are re-homed (into
/// `extra_members` / `extra_textures`); in/out names are aliased (`uv` →
/// `isf_FragNormCoord`, `fragColor` → `FragColor`, `texSampler`/`samp` → `img_sampler`).
/// Returns (body, needs_FragColor_out, dialect_detected). When the dialect is
/// detected, bool inputs must be declared `float` (baked blocks used float and
/// bodies compare against floats, e.g. `invert_r > 0.5`).
fn strip_baked_prelude(
    body: &str,
    provided: &[&str],
    texture_names: &[String],
    extra_members: &mut Vec<MemberDecl>,
    extra_textures: &mut Vec<String>,
) -> (String, bool, bool) {
    if !body.contains("ISFUniforms") {
        return (body.to_string(), false, false);
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut aliases: Vec<(String, &str)> = Vec::new();
    let mut needs_out = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if t.starts_with("layout") && t.contains("uniform") {
            if t.contains('{') {
                // uniform block: consume through the closing `}` line
                let mut members: Vec<&str> = Vec::new();
                i += 1;
                while i < lines.len() && !lines[i].contains('}') {
                    members.push(lines[i]);
                    i += 1;
                }
                i += 1; // skip the `};` line
                for m in members {
                    // members may be packed several per line: `float a; float b;`
                    for frag in m.split(';') {
                        let decl = frag.trim();
                        if decl.is_empty() {
                            continue;
                        }
                        let toks: Vec<&str> = decl.split_whitespace().collect();
                        let (name, ty) = match (toks.last(), toks.first()) {
                            (Some(n), Some(t)) => (*n, *t),
                            _ => continue,
                        };
                        if provided.contains(&name) {
                            continue; // we already declare it (builtin or header input)
                        }
                        let fty = match ty {
                            "float" => FieldTy::F32,
                            "int" | "uint" => FieldTy::I32,
                            "bool" => FieldTy::Bool,
                            "vec2" => FieldTy::Vec2,
                            "vec3" => FieldTy::Vec3,
                            "vec4" => FieldTy::Vec4,
                            _ => FieldTy::F32,
                        };
                        extra_members.push(MemberDecl {
                            name: name.to_string(),
                            glsl_decl: ty.to_string(),
                            fty,
                        });
                    }
                }
                continue;
            }
            // single-line layout-qualified opaque decl: `layout(...) uniform <ty> <name>;`
            let decl = t.trim_end_matches(';');
            let toks: Vec<&str> = decl.split_whitespace().collect();
            let ty = toks.get(toks.len().wrapping_sub(2)).copied();
            let name = toks.last().copied();
            if let (Some(name), Some(ty)) = (name, ty) {
                match ty {
                    "sampler" => {
                        aliases.push((name.to_string(), "img_sampler"));
                        i += 1;
                        continue;
                    }
                    "texture2D" | "sampler2D" => {
                        if !texture_names.iter().any(|n| n == name)
                            && !extra_textures.iter().any(|n| n == name)
                        {
                            extra_textures.push(name.to_string());
                        }
                        i += 1;
                        continue;
                    }
                    _ => {}
                }
            }
        } else if t.starts_with("layout") && t.contains(')') && {
            let rest = &t[t.find(')').unwrap() + 1..];
            let rest = rest.trim_start();
            rest.starts_with("in ") || rest.starts_with("out ")
        } {
            // `layout(location = N) in vec2 uv;` / `out vec4 fragColor;`
            let rest = t[t.find(')').unwrap() + 1..].trim_start();
            let is_out = rest.starts_with("out ");
            let decl = rest.trim_start_matches("in ").trim_start_matches("out ");
            let decl = decl.trim_end_matches(';');
            if let Some(name) = decl.split_whitespace().last() {
                if is_out {
                    aliases.push((name.to_string(), "FragColor"));
                    needs_out = true;
                } else {
                    aliases.push((name.to_string(), "isf_FragNormCoord"));
                }
                i += 1;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    let mut body = out;
    for (from, to) in aliases {
        body = replace_word(&body, &from, to);
    }
    (body, needs_out, true)
}

/// word-boundary identifier replacement (no regex dep).
fn replace_word(src: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find(from) {
        let before_ok = rest[..i]
            .chars()
            .last()
            .map(|c| !is_ident(c))
            .unwrap_or(true);
        let after = &rest[i + from.len()..];
        let after_ok = after.chars().next().map(|c| !is_ident(c)).unwrap_or(true);
        if before_ok && after_ok {
            out.push_str(&rest[..i]);
            out.push_str(to);
        } else {
            out.push_str(&rest[..i + from.len()]);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}
fn strip_header(src: &str) -> Result<String, String> {
    let src = src.trim_start_matches('\u{feff}');
    let start = src
        .find("/*")
        .ok_or_else(|| "missing ISF /*{...}*/ header comment".to_string())?;
    let end = src[start..]
        .find("*/")
        .ok_or_else(|| "unterminated ISF header comment".to_string())?
        + start;
    Ok(src[end + 2..].to_string())
}

/// Remove `#ifndef GL_ES`-guarded blocks that redefine GLSL builtins (the stock-corpus
/// `GLSL_450_INCOMPATIBLE` class: custom `distance`, `round`, etc.). When the guard has
/// an `#else` branch, that branch's content is kept (it is the desktop-GL branch).
fn excise_gles_builtin_redefs(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut keep = vec![true; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("#ifndef GL_ES") {
            // find matching #else / #endif at the same nesting depth
            let mut depth = 1i32;
            let mut else_idx = None;
            let mut endif_idx = None;
            for (j, line) in lines.iter().enumerate().skip(i + 1) {
                let t = line.trim_start();
                if t.starts_with("#if") {
                    depth += 1;
                } else if t.starts_with("#endif") {
                    depth -= 1;
                    if depth == 0 {
                        endif_idx = Some(j);
                        break;
                    }
                } else if t.starts_with("#else") && depth == 1 {
                    else_idx = Some(j);
                }
            }
            if let Some(endif) = endif_idx {
                let branch_end = else_idx.unwrap_or(endif);
                let redefines_builtin = lines[i + 1..branch_end].iter().any(|l| {
                    let t = l.trim_start();
                    REDEFINED_BUILTINS.iter().any(|name| {
                        ["float", "vec2", "vec3", "vec4", "int", "bool", "void"]
                            .iter()
                            .any(|ty| {
                                t.strip_prefix(&format!("{ty} {name}"))
                                    .map(|rest| rest.trim_start().starts_with('('))
                                    .unwrap_or(false)
                            })
                    })
                });
                if redefines_builtin {
                    for k in keep.iter_mut().take(branch_end).skip(i) {
                        *k = false;
                    }
                    if let Some(e) = else_idx {
                        keep[e] = false; // drop the #else line, keep its branch content
                    }
                    keep[endif] = false;
                }
                i = endif + 1;
                continue;
            }
        }
        i += 1;
    }
    lines
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(l, _)| *l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop global-scope non-opaque `uniform` declarations from the body. Redeclarations of
/// prelude-provided names vanish; unknown uniforms are appended to `extra_members` for
/// re-homing in the IsfInputs block.
fn dedup_uniforms(
    body: &str,
    provided: &[&str],
    extra_members: &mut Vec<MemberDecl>,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut depth = 0i32;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let mut drop_line = false;
        if depth == 0
            && !trimmed.starts_with("//")
            && let Some(rest) = trimmed.strip_prefix("uniform ")
            && let Some(end) = rest.find(';')
        {
                    let decl = &rest[..end];
                    let ty = decl.split_whitespace().next().unwrap_or("");
                    let opaque = ["sampler", "texture", "image", "subpass"]
                        .iter()
                        .any(|p| ty.starts_with(p));
                    if !opaque && !decl.contains('(') {
                        for d in decl[ty.len()..].split(',') {
                            let d = d.trim();
                            let name: String =
                                d.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                            if name.is_empty() {
                                continue;
                            }
                            if provided.contains(&name.as_str())
                                || extra_members.iter().any(|m| m.name == name)
                            {
                                continue;
                            }
                            // keep the original GLSL decl (incl. any [N] array suffix;
                            // initializers are dropped). See MemberDecl.fty for the
                            // std140 caveat on exotic re-homed types.
                            let suffix = match d.find('[') {
                                Some(b) => &d[b..d.find(']').map(|e| e + 1).unwrap_or(d.len())],
                                None => "",
                            };
                            let fty = match (ty, suffix.is_empty()) {
                                ("float", true) => FieldTy::F32,
                                ("int", true) => FieldTy::I32,
                                ("bool", true) => FieldTy::Bool,
                                ("vec2", true) => FieldTy::Vec2,
                                ("vec3", true) => FieldTy::Vec3,
                                ("vec4", true) => FieldTy::Vec4,
                                _ => FieldTy::F32,
                            };
                            extra_members.push(MemberDecl {
                                name,
                                glsl_decl: format!("{ty}{suffix}"),
                                fty,
                            });
                        }
                        drop_line = true;
                    }
        }
        if !drop_line {
            out.push_str(line);
            out.push('\n');
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
    }
    out
}

/// Assign explicit `layout(location=K)` to global-scope `varying`/`in`/`out`
/// declarations — Vulkan requires locations on all user IO.
fn assign_io_locations(body: &str, out_loc_start_at_1: bool) -> String {
    let mut in_loc = 1u32;
    let mut out_loc = if out_loc_start_at_1 { 1 } else { 0 };
    let mut depth = 0i32;
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let trimmed = line.trim_start();
        let at_global = depth == 0;
        let mut emit = line.to_string();
        let decl_end = trimmed.find(';');
        let decl_ok = decl_end
            .map(|e| {
                let decl = &trimmed[..e];
                let after = trimmed[e + 1..].trim();
                !decl.contains('(') && !decl.contains(')') && (after.is_empty() || after.starts_with("//"))
            })
            .unwrap_or(false);
        if at_global && decl_ok {
            let e = decl_end.unwrap();
            let decl = &trimmed[..e];
            let after = &trimmed[e..]; // ";" plus any trailing comment
            let indent = &line[..line.len() - trimmed.len()];
            if let Some(rest) = decl.strip_prefix("varying ") {
                emit = format!("{indent}layout(location = {in_loc}) in {rest}{after}");
                in_loc += array_slots(rest);
            } else if let Some(rest) = decl.strip_prefix("in ") {
                emit = format!("{indent}layout(location = {in_loc}) in {rest}{after}");
                in_loc += array_slots(rest);
            } else if let Some(rest) = decl.strip_prefix("out ") {
                emit = format!("{indent}layout(location = {out_loc}) out {rest}{after}");
                out_loc += array_slots(rest);
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        out.push_str(&emit);
        out.push('\n');
    }
    out
}

/// How many locations a global in/out declaration occupies (arrays take one per element).
fn array_slots(decl: &str) -> u32 {
    match decl.find('[').and_then(|i| decl[i + 1..].find(']').map(|j| &decl[i + 1..i + 1 + j])) {
        Some(n) => n.trim().parse().unwrap_or(1),
        None => 1,
    }
}

/// Inline ISF IMG_* builtins as texture expressions, Y-flip aware (see module docs).
///
/// naga spv-in rejects sampler2D function parameters, so these cannot be GLSL helper
/// functions; a balanced-paren textual inline is the smallest working approach.
fn inline_img_calls(body: &str) -> String {
    fn render(name: &str, args: &[String]) -> Option<String> {
        match (name, args.len()) {
            ("IMG_NORM_PIXEL", 2) => Some(format!(
                "texture(sampler2D({}, img_sampler), vec2(({}).x, 1.0 - ({}).y))",
                args[0], args[1], args[1]
            )),
            ("IMG_PIXEL", 2) => Some(format!(
                "texelFetch(sampler2D({}, img_sampler), ivec2(int(({}).x), int(RENDERSIZE.y) - 1 - int(({}).y)), 0)",
                args[0], args[1], args[1]
            )),
            ("IMG_SIZE", 1) => Some(format!("textureSize(sampler2D({}, img_sampler), 0)", args[0])),
            ("IMG_THIS_NORM_PIXEL", 1) => Some(format!(
                "texture(sampler2D({}, img_sampler), vec2((isf_FragNormCoord).x, 1.0 - (isf_FragNormCoord).y))",
                args[0]
            )),
            ("IMG_THIS_PIXEL", 1) => Some(format!(
                "texelFetch(sampler2D({}, img_sampler), ivec2(int((isf_FragNormCoord).x * RENDERSIZE.x), int(RENDERSIZE.y) - 1 - int((isf_FragNormCoord).y * RENDERSIZE.y)), 0)",
                args[0]
            )),
            _ => None,
        }
    }
    const NAMES: [&str; 5] = [
        "IMG_THIS_NORM_PIXEL",
        "IMG_THIS_PIXEL",
        "IMG_NORM_PIXEL",
        "IMG_PIXEL",
        "IMG_SIZE",
    ];
    let mut s = body.to_string();
    // loop for nested calls (e.g. IMG_NORM_PIXEL(t, IMG_SIZE(t)))
    for _ in 0..10 {
        let mut changed = false;
        let mut out: Vec<u8> = Vec::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < s.len() {
            let matched = NAMES.iter().find(|n| {
                bytes[i..].starts_with(n.as_bytes())
                    && (i == 0 || !is_ident(bytes[i - 1] as char))
                    && {
                        let after = &bytes[i + n.len()..];
                        after.first() == Some(&b'(')
                            || after.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'(')
                    }
            });
            match matched {
                Some(name) => {
                    let open = s[i..].find('(').unwrap() + i;
                    let (close, args) = match parse_call_args(&s, open) {
                        Some(x) => x,
                        None => {
                            out.extend_from_slice(&bytes[i..open + 1]);
                            i = open + 1;
                            continue;
                        }
                    };
                    match render(name, &args) {
                        Some(rep) => {
                            out.extend_from_slice(rep.as_bytes());
                            i = close + 1;
                            changed = true;
                        }
                        None => {
                            // copy just the name; keep scanning inside the parens
                            out.extend_from_slice(name.as_bytes());
                            i += name.len();
                        }
                    }
                }
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        }
        s = String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        if !changed {
            break;
        }
    }
    s
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Given `s` with `s[open] == '('`, return (index of matching ')', top-level comma-split args).
fn parse_call_args(s: &str, open: usize) -> Option<(usize, Vec<String>)> {
    let mut depth = 0i32;
    let mut args = Vec::new();
    let mut arg_start = open + 1;
    let bytes = s.as_bytes();
    let mut i = open;
    while i < s.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let last = s[arg_start..i].trim();
                    if !last.is_empty() || !args.is_empty() {
                        args.push(last.to_string());
                    }
                    return Some((i, args));
                }
            }
            b',' if depth == 1 => {
                args.push(s[arg_start..i].trim().to_string());
                arg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Find the user's `void main(` definition (word-boundary, paren after).
fn find_main_def(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = body[search_from..].find("void main") {
        let i = search_from + rel;
        let before_ok = i == 0 || !is_ident(bytes[i - 1] as char);
        let after = &body[i + "void main".len()..];
        let paren = after.trim_start().starts_with('(');
        if before_ok && paren {
            return Some(i);
        }
        search_from = i + 1;
    }
    None
}

/// Rename the user's `void main(` definition to `void isf_user_main(`.
fn rename_main(body: &str) -> String {
    match find_main_def(body) {
        Some(i) => format!(
            "{}void isf_user_main{}",
            &body[..i],
            &body[i + "void main".len()..]
        ),
        None => body.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Public entry point (API-compatible with the deleted textual transpiler)
// ---------------------------------------------------------------------------

/// Legacy uniform-array size for the vestigial `IsfUniforms` Pod type.
pub const MAX_ISF_UNIFORMS: usize = 64;

/// Map of ISF input name → index in the legacy flat uniform array (vestigial —
/// new code should use [`IsfManifest`]).
pub type UniformIndex = Vec<(String, usize)>;

/// Result of transpilation: the WGSL source, the manifest, and legacy fields.
pub struct Transpiled {
    pub wgsl: String,
    pub uniform_index: UniformIndex,
    /// True when the ISF shader has at least one image input, or uses IMG sampling macros.
    pub has_image_input: bool,
    /// GPU manifest (bind group layout + std140 inputs map + frag entry name).
    pub manifest: IsfManifest,
}

/// Transpile ISF GLSL source to WGSL via the prelude-GLSL + shaderc + naga pipeline
/// ([`compile`]). `uniform_index` is vestigial (legacy flat-slot layout).
pub fn generate_wgsl(isf: &Isf, glsl_src: &str) -> Result<Transpiled, String> {
    let out = compile(isf, glsl_src)?;

    // Legacy fields, kept for API compatibility.
    let glsl_body = strip_header(glsl_src).unwrap_or_default();
    let has_image_input = isf.inputs.iter().any(|i| {
        matches!(
            i.ty,
            InputType::Image | InputType::Audio(_) | InputType::AudioFft(_)
        )
    }) || [
        "IMG_NORM_PIXEL",
        "IMG_PIXEL",
        "IMG_THIS_PIXEL",
        "IMG_THIS_NORM_PIXEL",
    ]
    .iter()
    .any(|m| glsl_body.contains(m));
    let mut uniform_index: UniformIndex = Vec::new();
    let mut idx = 0usize;
    for input in &isf.inputs {
        match &input.ty {
            InputType::Float(_) | InputType::Bool(_) | InputType::Long(_) | InputType::Event => {
                uniform_index.push((input.name.clone(), idx));
                idx += 1;
            }
            InputType::Point2d(_) => {
                uniform_index.push((format!("{}_x", input.name), idx));
                uniform_index.push((format!("{}_y", input.name), idx + 1));
                idx += 2;
            }
            InputType::Color(_) => {
                uniform_index.push((format!("{}_r", input.name), idx));
                uniform_index.push((format!("{}_g", input.name), idx + 1));
                uniform_index.push((format!("{}_b", input.name), idx + 2));
                uniform_index.push((format!("{}_a", input.name), idx + 3));
                idx += 4;
            }
            _ => {}
        }
    }

    Ok(Transpiled {
        wgsl: out.wgsl,
        uniform_index,
        has_image_input,
        manifest: out.manifest,
    })
}
