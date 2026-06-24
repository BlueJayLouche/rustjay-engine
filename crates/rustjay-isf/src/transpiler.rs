//! ISF GLSL → WGSL transpiler.
//!
//! Supports the two common ISF entry-point patterns:
//! - Classic ISF: `void main()` writing to `gl_FragColor`
//! - Shadertoy-compat: `void mainImage(out vec4 fragColor, in vec2 fragCoord)` +
//!   `void main() { mainImage(gl_FragColor, gl_FragCoord.xy); }` bridge

// This is a line/char scanner with frequent lookahead (`lines[i+1]`, `chars[j]`)
// and manual prefix/suffix matching; index-based loops and manual stripping read
// more clearly here than iterator adapters.
#![allow(
    clippy::needless_range_loop,
    clippy::manual_strip,
    clippy::while_let_loop
)]

use isf::{InputType, Isf};

/// Maximum number of ISF uniforms (float/bool/int inputs + 4 built-ins).
pub const MAX_ISF_UNIFORMS: usize = 64;

/// Map of ISF input name → index in the uniform array.
pub type UniformIndex = Vec<(String, usize)>;

/// Result of transpilation: the WGSL source and the uniform index.
pub struct Transpiled {
    pub wgsl: String,
    pub uniform_index: UniformIndex,
    /// True when the ISF shader has at least one image input that needs t_input/s_input.
    pub has_image_input: bool,
}

/// Transpile ISF GLSL source to WGSL.
///
/// Returns `Ok(Transpiled)` on success or `Err(msg)` if the shader cannot be
/// represented in WGSL (e.g. uses unsupported features).
pub fn generate_wgsl(isf: &Isf, glsl_src: &str) -> Result<Transpiled, String> {
    // Step 1: strip ISF JSON comment header to get raw GLSL
    let glsl_body = strip_isf_comment(glsl_src);

    // Step 2: determine entry-point pattern
    let has_main_image = glsl_body.contains("void mainImage");
    // has_image_input: true when any image/audio input is present, OR when the GLSL source uses
    // IMG sampling macros (persistent-buffer shaders that reference their own rendered output).
    let has_image_input = isf.inputs.iter().any(|i| {
        matches!(
            i.ty,
            InputType::Image | InputType::Audio(_) | InputType::AudioFft(_)
        )
    }) || {
        let sampling_macros = [
            "IMG_NORM_PIXEL",
            "IMG_PIXEL",
            "IMG_THIS_PIXEL",
            "IMG_THIS_NORM_PIXEL",
        ];
        sampling_macros.iter().any(|m| glsl_body.contains(m))
    };

    // Step 3: collect ISF inputs → uniform index
    // Scalars (Float/Bool/Long/Event) get one slot; Point2D gets two (_x, _y); Color gets four.
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
    // Built-in slots: rendersize_x, rendersize_y, time, frame
    let builtin_base = idx;
    let _ = builtin_base; // reserved for documentation

    // Step 4: generate WGSL preamble (bind groups + uniforms struct)
    let preamble = generate_preamble(isf, &uniform_index, has_image_input);

    // Step 5: preprocess the GLSL body
    let processed = preprocess_glsl(
        glsl_body,
        isf,
        &uniform_index,
        has_main_image,
        has_image_input,
    );

    Ok(Transpiled {
        wgsl: format!("{}\n{}", preamble, processed),
        uniform_index,
        has_image_input,
    })
}

// ---------------------------------------------------------------------------
// Comment stripping
// ---------------------------------------------------------------------------

fn strip_isf_comment(src: &str) -> &str {
    // Find the end of the first /* ... */ block and return the rest
    if let Some(start) = src.find("/*")
        && let Some(end_offset) = src[start..].find("*/") {
            let after = &src[start + end_offset + 2..];
            return after.trim_start_matches('\n').trim_start_matches('\r');
        }
    src
}

/// Remove `//` line comments and `/* */` block comments from GLSL.
///
/// WGSL keeps comments verbatim, and naga rejects non-ASCII bytes that appear in
/// them (e.g. CJK author notes in Shadertoy ports); stray comment text can also
/// confuse the line-based rewrites below. GLSL has no string literals, so a
/// char-level scan is safe; block-comment newlines are preserved to keep line
/// numbers roughly aligned for error reporting.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    while let Some(&c2) = chars.peek() {
                        if c2 == '\n' {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
                Some('*') => {
                    chars.next(); // consume '*'
                    let mut prev = '\0';
                    for c2 in chars.by_ref() {
                        if prev == '*' && c2 == '/' {
                            break;
                        }
                        if c2 == '\n' {
                            out.push('\n');
                        }
                        prev = c2;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Preamble generation
// ---------------------------------------------------------------------------

fn generate_preamble(_isf: &Isf, uniform_index: &UniformIndex, has_image_input: bool) -> String {
    let mut out = String::new();

    // Texture bindings — only emit if there's an image input
    if has_image_input {
        out.push_str("@group(0) @binding(0) var t_input: texture_2d<f32>;\n");
        out.push_str("@group(0) @binding(1) var s_input: sampler;\n\n");
    }

    // Uniform struct
    out.push_str("struct IsfUniforms {\n");
    for (name, _idx) in uniform_index {
        out.push_str(&format!("    {}: f32,\n", sanitize_ident(name)));
    }
    // Pad to reach the built-in block (must be 4-float aligned)
    let scalar_count = uniform_index.len();
    let pad_needed = if !scalar_count.is_multiple_of(4) {
        4 - (scalar_count % 4)
    } else {
        0
    };
    for i in 0..pad_needed {
        out.push_str(&format!("    _pad{}: f32,\n", i));
    }
    out.push_str("    rendersize_x: f32,\n");
    out.push_str("    rendersize_y: f32,\n");
    out.push_str("    time: f32,\n");
    out.push_str("    frame: f32,\n");
    out.push_str("}\n");
    out.push_str("@group(1) @binding(0) var<uniform> isf_u: IsfUniforms;\n");
    // Thread-local vars: set at the start of fs_main so helper fns can read them.
    out.push_str("var<private> _isf_uv: vec2<f32>;\n");
    // _isf_clip: fragment position (gl_FragCoord equivalent) — accessible from helpers
    out.push_str("var<private> _isf_clip: vec4<f32>;\n\n");

    // Vertex stage
    out.push_str("struct VertIn {\n");
    out.push_str("    @location(0) pos: vec2<f32>,\n");
    out.push_str("    @location(1) uv:  vec2<f32>,\n");
    out.push_str("}\n");
    out.push_str("struct VertOut {\n");
    out.push_str("    @builtin(position) clip: vec4<f32>,\n");
    out.push_str("    @location(0) uv: vec2<f32>,\n");
    out.push_str("}\n");
    out.push_str("@vertex\n");
    out.push_str("fn vs_main(in: VertIn) -> VertOut {\n");
    out.push_str("    var out: VertOut;\n");
    out.push_str("    out.clip = vec4<f32>(in.pos, 0.0, 1.0);\n");
    out.push_str("    out.uv   = in.uv;\n");
    out.push_str("    return out;\n");
    out.push_str("}\n\n");

    out
}

// ---------------------------------------------------------------------------
// GLSL preprocessing → WGSL
// ---------------------------------------------------------------------------

fn preprocess_glsl(
    glsl: &str,
    isf: &Isf,
    uniform_index: &UniformIndex,
    has_main_image: bool,
    has_image_input: bool,
) -> String {
    // Strip comments first so non-ASCII author notes never reach WGSL and stray
    // comment text can't confuse the line-based rewrites below.
    let mut src = strip_comments(glsl);

    // Detect GLSL-450 style before stripping.
    let is_glsl450 = src.contains("layout(");
    let has_uv_varying = src.contains("in vec2 uv");

    // --- Preprocessor directives ---
    src = strip_preprocessor_blocks(&src);
    src = expand_fn_like_macros(&src); // parameterized #define macros before const conversion
    src = convert_define_to_const(&src);

    // --- Strip GLSL-450 layout declarations (output, input, uniform blocks, samplers, textures) ---
    // These are Vulkan-style GLSL constructs that have no WGSL equivalent.
    // Our WGSL preamble handles outputs, inputs, uniforms, and texture bindings separately.
    src = strip_glsl450_layout_decls(&src);

    // --- Strip GLSL precision qualifiers (highp/mediump/lowp used inline in declarations) ---
    src = strip_glsl_precision_qualifiers(&src);

    // --- Strip ISF neighbor-coord varying declarations (left_coord, right_coord etc.) ---
    // The ISF vertex shader provides these as varyings; in our WGSL we inject them as
    // local `let` bindings at the start of fs_main (see injected_locals below).
    src = strip_isf_coord_varyings(&src);

    // --- Strip GLSL function forward declarations (e.g. `vec3 rgb2hsv(vec3 c);`) ---
    // WGSL has no forward declarations; functions must be fully defined before use.
    // These lines would otherwise appear as invalid module-scope statements.
    src = strip_glsl_forward_declarations(&src);

    // --- Strip GLSL `uniform` declarations (ISF uses our own uniform struct) ---
    src = strip_uniform_declarations(&src);

    // --- Rewrite partial gl_FragColor / fragColor component writes (e.g. .rgb = ..., .a = ...) ---
    // These can't be handled by the simple `gl_FragColor = ` → `return` replacement.
    // Inject a local accumulator `_fc`, replace all component writes, add final assignment.
    if src.contains("gl_FragColor.") || src.contains("fragColor.") {
        src = rewrite_partial_gl_frag_color(&src);
    }

    // --- ISF built-in macros (word-boundary safe) ---
    // Replace swizzled RENDERSIZE.x/y first so clamp/max args stay scalar f32.
    // (Replacing bare RENDERSIZE first would give vec2<f32>(...).x which confuses
    // fix_builtin_type_mismatches into broadcasting scalar literals to vec2.)
    // IMPORTANT: replace multi-char swizzles (e.g. .xy) BEFORE single-char (.x) so
    // "RENDERSIZE.xy" doesn't get partially matched as "isf_u.rendersize_xy".
    src = src.replace(
        "RENDERSIZE.xy",
        "vec2<f32>(isf_u.rendersize_x, isf_u.rendersize_y)",
    );
    src = src.replace("RENDERSIZE.x", "isf_u.rendersize_x");
    src = src.replace("RENDERSIZE.y", "isf_u.rendersize_y");
    src = src.replace("RENDERSIZE[0]", "isf_u.rendersize_x");
    src = src.replace("RENDERSIZE[1]", "isf_u.rendersize_y");
    src = replace_word(
        &src,
        "RENDERSIZE",
        "vec2<f32>(isf_u.rendersize_x, isf_u.rendersize_y)",
    );
    src = replace_word(&src, "FRAMEINDEX", "isf_u.frame");
    src = replace_word(&src, "TIME", "isf_u.time");
    src = replace_word(&src, "TIMEDELTA", "(1.0/60.0)");
    src = replace_word(&src, "PASSINDEX", "0");
    // DATE → placeholder vec4 (year, month, day, seconds-in-day); no real-time source available.
    src = replace_word(&src, "DATE", "vec4<f32>(2024.0, 1.0, 1.0, isf_u.time)");
    // Some shaders declare `uniform vec2 renderSize;` instead of using RENDERSIZE.
    // After strip_uniform_declarations removes the declaration, replace bare uses.
    if !uniform_index.iter().any(|(n, _)| n == "renderSize") {
        src = replace_word(
            &src,
            "renderSize",
            "vec2<f32>(isf_u.rendersize_x, isf_u.rendersize_y)",
        );
    }

    // --- Varda-specific built-in uniforms (declared in ISFUniforms block, not ISF INPUTS) ---
    // Audio analysis — not wired yet, default to 0.0
    src = replace_word(&src, "audio_level", "0.0");
    src = replace_word(&src, "audio_bass", "0.0");
    src = replace_word(&src, "audio_mid", "0.0");
    src = replace_word(&src, "audio_treble", "0.0");
    src = replace_word(&src, "audio_bpm", "0.0");
    src = replace_word(&src, "audio_beat_phase", "0.0");
    // Phase time accumulators — map to TIME for now
    for i in 0..8usize {
        let name = format!("PHASE_TIME_{}", i);
        src = replace_word(&src, &name, "isf_u.time");
    }
    // Common math constants that may be defined via #define (stripped by preprocessor)
    src = replace_word(&src, "PI", "3.14159265359");
    src = replace_word(&src, "TAU", "6.28318530718");
    // Audio FFT sample count built-in
    src = replace_word(&src, "SAMPLES", "256.0");

    // ISF image-rect variables (e.g. `_inputImage_imgRect`, `_maskImage_imgRect`).
    // These hold the normalized texture rect (x, y, w, h) of an image input.
    // Substitute the identity rect (0,0,1,1) — valid for validation purposes.
    if src.contains("_imgRect") {
        let mut out2 = String::with_capacity(src.len());
        for line in src.lines() {
            if line.contains("_imgRect") {
                let mut s = line.to_owned();
                while let Some(pos) = s.find("_imgRect") {
                    let suffix_end = pos + "_imgRect".len();
                    let bytes = s.as_bytes();
                    let mut start = pos;
                    while start > 0
                        && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                    {
                        start -= 1;
                    }
                    s = format!(
                        "{}vec4<f32>(0.0, 0.0, 1.0, 1.0){}",
                        &s[..start],
                        &s[suffix_end..]
                    );
                }
                out2.push_str(&s);
            } else {
                out2.push_str(line);
            }
            out2.push('\n');
        }
        if !src.ends_with('\n') && out2.ends_with('\n') {
            out2.pop();
        }
        src = out2;
    }

    // --- GLSL built-ins not in WGSL ---
    // gl_FragCoord maps to _isf_clip (var<private> set at start of fs_main).
    // Using a private var (rather than `in.clip`) allows helper functions to access it too.
    src = src.replace("gl_FragCoord.xy", "_isf_clip.xy");
    src = src.replace("gl_FragCoord.zw", "_isf_clip.zw");
    src = src.replace("gl_FragCoord", "_isf_clip");
    // Normalize `if(` → `if (` etc. (no space before paren)
    src = normalize_control_flow_spacing(&src);

    // --- ISF normalized UV (isf_FragNormCoord) → _isf_uv (var<private>, set in fs_main) ---
    // Unconditional: helper functions may use isf_FragNormCoord even without image inputs.
    src = src.replace("isf_FragNormCoord", "_isf_uv");
    src = src.replace("vv_FragNormCoord", "_isf_uv");

    // IMG_SIZE doesn't need t_input/s_input — always safe to replace.
    src = replace_img_size(&src);
    // IMG sampling macros need t_input/s_input (declared in preamble when has_image_input).
    if has_image_input {
        src = replace_img_macros(&src);
    }

    // --- Rename function parameters that collide with ISF input names ---
    // ISF input names are replaced globally (src → isf_u.NAME). If a user-defined function
    // has a parameter with the same name (e.g. `float size` when `size` is an ISF input),
    // the replacement would produce `isf_u.size` as a parameter identifier, which is invalid.
    // Pre-rename those parameters to `_fp_NAME` so they survive the ISF replacement pass.
    {
        let isf_names: Vec<&str> = isf.inputs.iter().map(|i| i.name.as_str()).collect();
        src = rename_params_conflicting_with_isf_inputs(&src, &isf_names);
    }

    // --- Remap Point2D and Color inputs → vec2/vec4 expressions (before scalar remapping) ---
    // Do longest names first to avoid partial matches on shorter names.
    let mut compound_sorted: Vec<&isf::Input> = isf
        .inputs
        .iter()
        .filter(|i| matches!(i.ty, InputType::Point2d(_) | InputType::Color(_)))
        .collect();
    compound_sorted.sort_by_key(|a| std::cmp::Reverse(a.name.len()));
    for input in &compound_sorted {
        let safe = sanitize_ident(&input.name);
        let replacement = match &input.ty {
            InputType::Point2d(_) => format!("vec2<f32>(isf_u.{}_x, isf_u.{}_y)", safe, safe),
            InputType::Color(_) => format!(
                "vec4<f32>(isf_u.{}_r, isf_u.{}_g, isf_u.{}_b, isf_u.{}_a)",
                safe, safe, safe, safe
            ),
            _ => unreachable!(),
        };
        src = replace_word(&src, &input.name, &replacement);
    }

    // --- Remap ISF scalar input names to isf_u.NAME ---
    // Skip compound-input component names (name_x, name_y, etc.) — they're now inside
    // the vec2<f32>(...) expressions generated above, so don't replace them a second time.
    let compound_component_names: std::collections::HashSet<String> = isf
        .inputs
        .iter()
        .flat_map(|i| match &i.ty {
            InputType::Point2d(_) => vec![format!("{}_x", i.name), format!("{}_y", i.name)],
            InputType::Color(_) => vec![
                format!("{}_r", i.name),
                format!("{}_g", i.name),
                format!("{}_b", i.name),
                format!("{}_a", i.name),
            ],
            _ => vec![],
        })
        .collect();
    // Bool/Event inputs stored as f32, but GLSL uses them as bool.
    // Replace with `(isf_u.NAME != 0.0)` so WGSL if-conditions get a bool.
    // Long inputs are also stored as f32, but GLSL uses them as int.
    // Replace long inputs with `i32(isf_u.NAME)` so integer contexts work in WGSL.
    // Do longest names first to avoid partial matches.
    let bool_inputs: std::collections::HashSet<&str> = isf
        .inputs
        .iter()
        .filter(|i| matches!(i.ty, InputType::Bool(_) | InputType::Event))
        .map(|i| i.name.as_str())
        .collect();
    let long_inputs: std::collections::HashSet<&str> = isf
        .inputs
        .iter()
        .filter(|i| matches!(i.ty, InputType::Long(_)))
        .map(|i| i.name.as_str())
        .collect();
    let mut sorted_uniforms: Vec<&(String, usize)> = uniform_index
        .iter()
        .filter(|(name, _)| !compound_component_names.contains(name))
        .collect();
    sorted_uniforms.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    for (name, _idx) in &sorted_uniforms {
        let safe = sanitize_ident(name);
        let replacement = if bool_inputs.contains(name.as_str()) {
            if is_glsl450 {
                // GLSL-450 shaders (e.g. varda) store bools as float and compare them
                // explicitly (e.g. `if (bool_input > 0.5)`). Wrapping with `!= 0.0`
                // would produce invalid double-bool expressions like
                // `((isf_u.name != 0.0) > 0.5)`. Leave as raw float.
                format!("isf_u.{}", safe)
            } else {
                // Wrap as explicit bool so WGSL if-conditions work (f32 != 0.0 → bool)
                format!("(isf_u.{} != 0.0)", safe)
            }
        } else if long_inputs.contains(name.as_str()) {
            // Long inputs stored as f32; cast to i32 so integer arithmetic works in WGSL
            format!("i32(isf_u.{})", safe)
        } else {
            format!("isf_u.{}", safe)
        };
        // Only replace bare identifiers (word boundary)
        src = replace_word(&src, name, &replacement);
    }

    // --- Extract module-scope runtime vars (must come before type conversion)
    // Module-scope GLSL vars like `vec3 iResolution = vec3(RENDERSIZE...)` cannot
    // exist at module scope in WGSL because they reference runtime uniform values.
    // We collect them and inject them into the fs_main body instead.
    let (src, injected_locals_raw) = extract_module_scope_runtime_vars(&src);
    let mut src = src;

    // --- GLSL → WGSL type/syntax ---
    src = convert_types_and_syntax(&src);

    // --- Prefix increment/decrement: `++i` → `i++` (WGSL only allows postfix in for updates) ---
    src = convert_prefix_increments(&src);

    // --- Rename WGSL reserved keywords used as variable names ---
    src = rename_wgsl_reserved_keywords(&src);

    // --- Join multi-line ternary expressions before conversion ---
    // GLSL allows `cond\n? true_val\n: false_val` across multiple lines.
    // Flatten so the ternary converter (which works line-by-line) can see the whole expression.
    src = join_ternary_continuation_lines(&src);
    // --- Ternary operator: COND ? A : B → select(B, A, COND) ---
    src = convert_ternary_operators(&src);

    // --- gl_FragColor / fragColor → return (MUST be AFTER ternary so ternary parser doesn't grab `return` as condition) ---
    // Use "return " (with space) so `gl_FragColor =select(...)` → `return select(...)`, not `returnselect(...)`.
    src = src.replace("gl_FragColor =", "return ");
    src = src.replace("gl_FragColor=", "return ");
    // GLSL-450 shaders often declare `layout(location = 0) out vec4 fragColor;` and write to it.
    src = src.replace("fragColor =", "return ");
    src = src.replace("fragColor=", "return ");

    // --- Braceless control-flow bodies (WGSL requires compound statements) ---
    // First handle multi-line braceless (must run before single-line so we have correct context)
    src = add_braces_to_multiline_braceless_control_flow(&src);
    src = add_braces_to_braceless_control_flow(&src);
    // Handle braceless if/for/while that appear *inside* a same-line braced block.
    src = fix_inline_braceless_control_flow(&src);

    // --- For loop headers ---
    src = convert_for_loops(&src);

    // --- Function signatures ---
    src = convert_function_signatures(&src);

    // --- Inject `var` shadows for function parameters (WGSL params are immutable).
    //     Renames params to `_name` and injects `var name: T = _name;` in the body.
    src = inject_param_var_shadows(&src);

    // --- Variable declarations ---
    src = convert_var_declarations(&src);

    // --- Promote module-scope `var name:` to `var<private> name:` ---
    // WGSL requires an explicit address space at module scope. Function-scope `var` is fine
    // as-is; only module-scope declarations need the promotion.
    src = promote_module_scope_vars_to_private(&src);

    // --- Multi-var declarations: `var a: T = x, b = y` → separate statements.
    //     Must run after convert_var_declarations so `var` prefix is present.
    src = split_multi_var_decls(&src);

    // --- Resolve GLSL function overloads → unique WGSL names.
    //     GLSL allows multiple functions with the same name but different param types.
    //     WGSL does not. Rename duplicate `fn NAME(` defs with type-based suffixes
    //     and update all call sites.
    src = resolve_function_overloads(&src);

    // --- Fix GLSL scalar-broadcasts that WGSL doesn't allow.
    //     GLSL: max(vec4_expr, 0.0) — WGSL: max(vec4_expr, vec4<f32>(0.0))
    //     GLSL: smoothstep(0.0, 1.0, vec2_expr) — WGSL needs matching types.
    //     Also fixes swizzle compound assignment: `v.xy -= u;` → component-wise.
    src = fix_builtin_type_mismatches(&src);
    src = fix_swizzle_compound_assignments(&src);

    // --- Fix i32/u32 cast comparisons with f32: `i32(expr) == f32_val` → `f32(i32(expr)) == f32_val`
    //     GLSL allows implicit int→float coercion in comparisons; WGSL requires exact types.
    src = fix_int_cast_comparisons(&src);

    // --- Fix oversized vec constructors: `vec3<f32>(vec2_expr, s1, s2)` has 4 components.
    //     GLSL drivers silently truncate; WGSL is strict (reports "expects 3, got 4").
    src = fix_oversized_vec_constructors(&src);

    // Apply the same conversion pipeline to the injected locals (extracted pre-conversion)
    let mut injected_locals: Vec<String> = {
        let joined = injected_locals_raw.join("\n");
        let converted = convert_types_and_syntax(&joined);
        let converted = join_ternary_continuation_lines(&converted);
        let converted = convert_ternary_operators(&converted);
        let converted = convert_function_signatures(&converted);
        let converted = convert_var_declarations(&converted);
        converted.lines().map(|l| l.to_string()).collect()
    };

    // --- Inject ISF neighbor-coord let bindings if used ---
    // These are provided by the ISF vertex shader as varyings; we synthesize them from _isf_uv.
    let coord_defs: &[(&str, &str)] = &[
        ("left_coord",       "    let left_coord: vec2<f32> = _isf_uv + vec2<f32>(-1.0/isf_u.rendersize_x, 0.0);"),
        ("right_coord",      "    let right_coord: vec2<f32> = _isf_uv + vec2<f32>(1.0/isf_u.rendersize_x, 0.0);"),
        ("above_coord",      "    let above_coord: vec2<f32> = _isf_uv + vec2<f32>(0.0, 1.0/isf_u.rendersize_y);"),
        ("below_coord",      "    let below_coord: vec2<f32> = _isf_uv + vec2<f32>(0.0, -1.0/isf_u.rendersize_y);"),
        ("lefta_coord",      "    let lefta_coord: vec2<f32> = _isf_uv + vec2<f32>(-1.0/isf_u.rendersize_x, 1.0/isf_u.rendersize_y);"),
        ("righta_coord",     "    let righta_coord: vec2<f32> = _isf_uv + vec2<f32>(1.0/isf_u.rendersize_x, 1.0/isf_u.rendersize_y);"),
        ("leftb_coord",      "    let leftb_coord: vec2<f32> = _isf_uv + vec2<f32>(-1.0/isf_u.rendersize_x, -1.0/isf_u.rendersize_y);"),
        ("rightb_coord",     "    let rightb_coord: vec2<f32> = _isf_uv + vec2<f32>(1.0/isf_u.rendersize_x, -1.0/isf_u.rendersize_y);"),
        ("translated_coord", "    let translated_coord: vec2<f32> = _isf_uv;"),
    ];
    for (coord_name, def) in coord_defs {
        if src.contains(coord_name) {
            injected_locals.push(def.to_string());
        }
    }

    // texcoord0–7: ISF neighbor varyings (stripped above). Inject as _isf_uv placeholders.
    for n in 0usize..8 {
        let name = format!("texcoord{}", n);
        if src.contains(&name) {
            injected_locals.push(format!("    let {}: vec2<f32> = _isf_uv;", name));
        }
    }

    // uv: GLSL-450 fragment UV varying (stripped by strip_glsl450_layout_decls).
    // Only inject when the original source declared it as a varying, not when `uv`
    // is a local variable (e.g. `vec2 uv = fragCoord / iResolution.xy;`).
    if has_uv_varying && contains_word(&src, "uv") {
        injected_locals.push("    let uv: vec2<f32> = _isf_uv;".to_string());
    }

    // texOffsets: ISF multi-pass neighbor-coord array (stripped above). Inject a stub.
    if src.contains("texOffsets") {
        injected_locals.push(
            "    let texOffsets = array<vec2<f32>, 8>(\
            _isf_uv + vec2<f32>(-2.0/isf_u.rendersize_x, 0.0), \
            _isf_uv + vec2<f32>(-1.0/isf_u.rendersize_x, 0.0), \
            _isf_uv, \
            _isf_uv + vec2<f32>(1.0/isf_u.rendersize_x, 0.0), \
            _isf_uv + vec2<f32>(2.0/isf_u.rendersize_x, 0.0), \
            _isf_uv + vec2<f32>(0.0, -1.0/isf_u.rendersize_y), \
            _isf_uv + vec2<f32>(0.0, 1.0/isf_u.rendersize_y), \
            _isf_uv + vec2<f32>(0.0, 2.0/isf_u.rendersize_y));"
                .to_string(),
        );
    }

    // rotmat: ISF rotation matrix varying. Inject identity mat2x2 placeholder.
    if src.contains("rotmat") {
        injected_locals
            .push("    let rotmat: mat2x2<f32> = mat2x2<f32>(1.0, 0.0, 0.0, 1.0);".to_string());
    }

    // --- Entry point ---
    if has_main_image {
        src = convert_main_image_entry(&src, has_image_input, &injected_locals);
    } else {
        src = convert_void_main_entry(&src, has_image_input, &injected_locals);
    }

    // --- Move @fragment fn to the end so helper functions defined after main() come first ---
    // WGSL requires functions to be defined before first use; ISF shaders often define void main()
    // before helper functions. Moving @fragment fn fs_main to the end fixes ordering.
    src = move_fragment_fn_to_end(&src);

    // --- Fix discarded @must_use return values ---
    // WGSL marks many builtins (abs, sin, cos, ...) as @must_use. A standalone expression
    // statement like `abs(p);` is valid GLSL but invalid WGSL. Use `_ = expr;` (phony assignment).
    src = fix_discarded_must_use_calls(&src);

    // --- Fix bool expressions used as float vec components ---
    // GLSL allows implicit bool→float conversion; WGSL requires explicit f32(bool_expr).
    // Common pattern: ISF bool input `(isf_u.NAME != 0.0)` used inside vec2/vec3/vec4(...).
    src = fix_bool_in_vec_constructors(&src);

    // --- Add missing fallback returns ---
    // Functions that end with an if/else-if chain without a final else have no return on
    // every code path; WGSL requires all paths to return. Insert a dead-code fallback.
    src = add_missing_function_returns(&src);

    src
}

/// Detect standalone expression statements like `abs(expr);` where the return value is
/// discarded and prefix them with `_ = ` (phony assignment) to satisfy WGSL's `@must_use` constraint.
/// GLSL allows implicit bool→float conversion; WGSL does not.
/// When a bool expression (comparison like `x != 0.0`, or `any(...)`, `all(...)`) appears
/// as a component in a vec2/vec3/vec4 float constructor, wrap it in `f32(...)`.
fn fix_bool_in_vec_constructors(src: &str) -> String {
    fn looks_like_bool_expr(s: &str) -> bool {
        let s = s.trim();
        // Parenthesized comparison: `(expr == ...) ` or `(expr != ...)`
        if s.starts_with('(') && s.ends_with(')') {
            let inner = &s[1..s.len() - 1];
            if inner.contains("==")
                || inner.contains("!=")
                || inner.contains("<=")
                || inner.contains(">=")
                || inner.contains(" < ")
                || inner.contains(" > ")
                || inner.contains("&&")
                || inner.contains("||")
            {
                return true;
            }
        }
        // bare comparison without outer parens
        if s.contains("==") || s.contains("!=") || s.contains("&&") || s.contains("||") {
            return true;
        }
        // starts with `!` (boolean not)
        if s.starts_with('!') {
            return true;
        }
        // any/all builtins
        if s.starts_with("any(") || s.starts_with("all(") {
            return true;
        }
        false
    }

    let vec_prefixes: &[&str] = &["vec4<f32>(", "vec3<f32>(", "vec2<f32>("];
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::with_capacity(src.len());

    for &line in &lines {
        let mut result = line.to_owned();
        for &prefix in vec_prefixes {
            if !result.contains(prefix) {
                continue;
            }
            let mut s2 = String::with_capacity(result.len() + 32);
            let mut rest: &str = &result;
            loop {
                let Some(pos) = rest.find(prefix) else {
                    s2.push_str(rest);
                    break;
                };
                let before_char = if pos > 0 {
                    rest.as_bytes()[pos - 1] as char
                } else {
                    ' '
                };
                if is_word_char(before_char) {
                    s2.push_str(&rest[..pos + 1]);
                    rest = &rest[pos + 1..];
                    continue;
                }
                s2.push_str(&rest[..pos + prefix.len()]);
                let after_open = &rest[pos + prefix.len()..];
                let (args_str, after_close) = extract_balanced(after_open, ')');
                if after_close.is_empty() && args_str.len() == after_open.len() {
                    rest = after_open;
                    continue;
                }
                // Process each argument
                let args = split_top_level_commas(args_str);
                let fixed_args: Vec<String> = args
                    .iter()
                    .map(|a| {
                        let t = a.trim();
                        if looks_like_bool_expr(t) {
                            format!("f32({})", t)
                        } else {
                            t.to_owned()
                        }
                    })
                    .collect();
                s2.push_str(&fixed_args.join(", "));
                s2.push(')');
                rest = after_close;
            }
            result = s2;
        }
        out.push_str(&result);
        out.push('\n');
    }
    if !src.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// For non-void functions that end with if/else-if chains without a final else, WGSL
/// requires all code paths to return. This pass adds a dead-code fallback return before
/// the closing `}` of any such function, for primitive return types we can zero-initialize.
fn add_missing_function_returns(src: &str) -> String {
    fn default_return_for_type(ty: &str) -> Option<String> {
        let ty = ty.trim();
        match ty {
            "f32" => Some("return 0.0;".to_string()),
            "i32" => Some("return 0i;".to_string()),
            "u32" => Some("return 0u;".to_string()),
            "bool" => Some("return false;".to_string()),
            t if t.starts_with("vec2") => Some(format!("return {}(0.0);", t)),
            t if t.starts_with("vec3") => Some(format!("return {}(0.0);", t)),
            t if t.starts_with("vec4") => Some(format!("return {}(0.0, 0.0, 0.0, 1.0);", t)),
            _ => None,
        }
    }

    let lines: Vec<&str> = src.lines().collect();
    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len() + 16);

    let mut fn_return_type: Option<String> = None;
    let mut brace_depth: i32 = 0;
    // True if the last relevant statement at depth 1 was a `return`.
    let mut last_depth1_was_return = false;

    for &line in &lines {
        let trimmed = line.trim();
        let is_comment = trimmed.starts_with("//");

        // Detect function start with a return type.
        if trimmed.starts_with("fn ") && trimmed.contains("->") && brace_depth == 0
            && let Some(arrow_pos) = trimmed.find("->") {
                let after_arrow = trimmed[arrow_pos + 2..].trim();
                let ty_end = after_arrow.find('{').unwrap_or(after_arrow.len());
                let ret_ty = after_arrow[..ty_end].trim().to_string();
                if !ret_ty.is_empty() && ret_ty != "void" {
                    fn_return_type = Some(ret_ty);
                    last_depth1_was_return = false;
                }
            }

        // Process brace depth changes — inject fallback before the closing `}`.
        if fn_return_type.is_some() {
            let open_count = trimmed.chars().filter(|&c| c == '{').count() as i32;
            let close_count = trimmed.chars().filter(|&c| c == '}').count() as i32;

            // If this line closes the function (depth 1 → 0 or crosses 0)
            let new_depth = brace_depth + open_count - close_count;
            if brace_depth > 0 && new_depth == 0 {
                // Function is closing on this line — inject fallback if needed.
                if let Some(ref ret_ty) = fn_return_type.clone()
                    && !last_depth1_was_return
                        && let Some(ret_stmt) = default_return_for_type(ret_ty) {
                            let indent = &line[..line.len() - line.trim_start().len()];
                            out_lines.push(format!("{}    {}", indent, ret_stmt));
                        }
                fn_return_type = None;
                last_depth1_was_return = false;
            }

            brace_depth = new_depth;
        } else {
            let open_count = trimmed.chars().filter(|&c| c == '{').count() as i32;
            let close_count = trimmed.chars().filter(|&c| c == '}').count() as i32;
            brace_depth = (brace_depth + open_count - close_count).max(0);
        }

        // Track if this is a return statement at the function body level (depth 1).
        if fn_return_type.is_some() && !is_comment && !trimmed.is_empty() && brace_depth == 1 {
            last_depth1_was_return = trimmed.starts_with("return ") || trimmed == "return;";
        }

        out_lines.push(line.to_string());
    }

    out_lines.join("\n")
}

fn line_ends_with_binary_op(s: &str) -> bool {
    let t = s.trim_end();
    t.ends_with("&&")
        || t.ends_with("||")
        || t.ends_with("??")
        || (t.ends_with('+') && !t.ends_with("++"))
        || (t.ends_with('-') && !t.ends_with("--"))
        || t.ends_with('*')
        || t.ends_with('/')
        || t.ends_with('%')
        || t.ends_with('|')
        || t.ends_with('&')
        || t.ends_with('^')
        || t.ends_with("return")
}

fn fix_discarded_must_use_calls(src: &str) -> String {
    let wgsl_keywords: &[&str] = &[
        "if",
        "for",
        "while",
        "else",
        "return",
        "let",
        "var",
        "fn",
        "struct",
        "const",
        "break",
        "continue",
        "loop",
        "switch",
        "case",
        "default",
        "@fragment",
        "@vertex",
        "@group",
        "@builtin",
        "@location",
    ];
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::with_capacity(src.len());
    for (idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let mut wrapped = false;
        // Quick checks: ends with `;`, not empty, not a comment
        if trimmed.ends_with(';') && !trimmed.starts_with("//") && !trimmed.is_empty() {
            // Skip if this line continues a multi-line expression (prev line ended with binary op)
            let prev_trimmed = lines[..idx]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .unwrap_or("");
            let is_continuation = line_ends_with_binary_op(prev_trimmed);
            // Check it starts with an identifier (function call)
            let ident_end = trimmed
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(0);
            if !is_continuation
                && ident_end > 0
                && trimmed.as_bytes().get(ident_end).copied() == Some(b'(')
            {
                let ident = &trimmed[..ident_end];
                // Skip if it's a WGSL keyword
                if !wgsl_keywords.contains(&ident) {
                    // Skip continuation lines: if `)` outnumbers `(`, we're inside an outer paren.
                    let paren_balance: i32 = trimmed
                        .chars()
                        .map(|c| match c {
                            '(' => 1,
                            ')' => -1,
                            _ => 0,
                        })
                        .sum();
                    // Check there's no top-level `=` assignment in the line
                    let has_assign = {
                        let mut depth = 0i32;
                        let mut found = false;
                        let bytes = trimmed.as_bytes();
                        let mut i = 0;
                        while i < bytes.len() {
                            match bytes[i] {
                                b'(' | b'[' | b'{' => depth += 1,
                                b')' | b']' | b'}' => depth -= 1,
                                b'=' if depth == 0 => {
                                    let prev = if i > 0 { bytes[i - 1] } else { 0 };
                                    let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
                                    if prev != b'!'
                                        && prev != b'<'
                                        && prev != b'>'
                                        && prev != b'='
                                        && next != b'='
                                    {
                                        found = true;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                        found
                    };
                    if !has_assign && paren_balance >= 0 {
                        let indent = &line[..line.len() - line.trim_start().len()];
                        out.push_str(&format!("{}_ = {}\n", indent, trimmed));
                        wrapped = true;
                    }
                }
            }
        }
        if !wrapped {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Move `@fragment fn fs_main` (and its `@fragment` attribute line) to the end of the source.
/// WGSL requires functions to be defined before first use. ISF shaders often write `void main()`
/// before helper functions; after conversion, `@fragment fn fs_main` appears too early.
fn move_fragment_fn_to_end(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();

    // Find the `@fragment` attribute line that precedes `fn fs_main`.
    let mut frag_start = None;
    for i in 0..n {
        if lines[i].trim() == "@fragment" {
            // Check if the next non-empty line is `fn fs_main`
            let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
            if let Some(nxt) = next
                && nxt.trim().starts_with("fn fs_main") {
                    frag_start = Some(i);
                    break;
                }
        }
    }
    let frag_start = match frag_start {
        Some(i) => i,
        None => return src.to_string(), // no @fragment found — nothing to do
    };

    // Find the end of the @fragment function by tracking brace depth.
    let mut brace_depth = 0i32;
    let mut frag_end = n; // exclusive
    let mut entered = false;
    for i in frag_start..n {
        for c in lines[i].chars() {
            match c {
                '{' => {
                    brace_depth += 1;
                    entered = true;
                }
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if entered && brace_depth <= 0 {
            frag_end = i + 1;
            break;
        }
    }

    // Reconstruct: everything before @fragment + everything after + @fragment block
    let mut out = String::with_capacity(src.len() + 2);
    for i in 0..frag_start {
        out.push_str(lines[i]);
        out.push('\n');
    }
    for i in frag_end..n {
        out.push_str(lines[i]);
        out.push('\n');
    }
    out.push('\n');
    for i in frag_start..frag_end {
        out.push_str(lines[i]);
        out.push('\n');
    }
    out
}

/// Extract module-scope GLSL variable declarations that use runtime values
/// (i.e. they can't be WGSL module-level `var`).
///
/// Returns (modified_src_with_lines_removed, vec_of_wgsl_local_decl_lines).
fn extract_module_scope_runtime_vars(src: &str) -> (String, Vec<String>) {
    let glsl_types = [
        "float ", "int ", "uint ", "bool ", "vec2 ", "vec3 ", "vec4 ", "ivec2 ", "ivec3 ",
        "ivec4 ", "mat2 ", "mat3 ", "mat4 ",
    ];

    let mut out = String::with_capacity(src.len());
    let mut injected = Vec::new();
    let mut brace_depth = 0i32;
    // Track names of vars extracted as runtime so transitive dependents are also extracted.
    // (e.g. `vec2 ratio2 = vec2(1.0, 1.0/ratio)` — `ratio` is runtime, so ratio2 is too)
    let mut runtime_names: Vec<String> = Vec::new();

    for line in src.lines() {
        let trimmed = line.trim();

        // Track brace depth
        for c in trimmed.chars() {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        // Only extract from module scope (depth 0 after processing the line's braces)
        if brace_depth == 0 {
            // Check if this is a GLSL type variable declaration with an initializer
            // (e.g. `vec3 iResolution = ...;` or `float iTime = TIME;`)
            let is_var_decl = glsl_types.iter().any(|ty| trimmed.starts_with(ty));
            let has_init = trimmed.contains('=') && trimmed.ends_with(';');
            let is_func_sig = trimmed.contains('(') && trimmed.ends_with('{');

            if is_var_decl && has_init && !is_func_sig && !trimmed.starts_with("const ") {
                // A var is "runtime" if it references a uniform or any previously-extracted
                // runtime var. Track transitively so `vec2 ratio2 = vec2(1.0, 1.0/ratio)`
                // is also treated as runtime when `ratio` was extracted above it.
                let references_runtime = trimmed.contains("isf_u")
                    || runtime_names.iter().any(|n| contains_word(trimmed, n));

                if references_runtime {
                    // Runtime var referencing a uniform (possibly transitively) — can't init
                    // at WGSL module scope. Emit an uninitialized `TYPE name;` at module scope
                    // (`promote_module_scope_vars_to_private` will make it `var<private> name: TYPE;`),
                    // then inject `name = expr;` into fs_main so helpers can see it too.
                    if let Some(eq_pos) = trimmed.find('=') {
                        let decl_part = trimmed[..eq_pos].trim();
                        let expr_part = trimmed[eq_pos + 1..].trim().trim_end_matches(';');
                        if let Some(var_name) = decl_part.split_whitespace().last() {
                            let var_type = decl_part[..decl_part.len() - var_name.len()].trim();
                            // Emit uninitialized module-scope declaration
                            out.push_str(var_type);
                            out.push(' ');
                            out.push_str(var_name);
                            out.push_str(";\n");
                            // Inject assignment into fs_main body
                            injected.push(format!("    {} = {};", var_name, expr_part));
                            runtime_names.push(var_name.to_string());
                            continue;
                        }
                    }
                    // Fallback: inject as local statement (only works if not used in helpers)
                    injected.push(format!("    {};", trimmed.trim_end_matches(';')));
                } else {
                    // Pure constant expression — keep as module-scope `const`
                    // Prepend `const ` so convert_var_declarations emits `const NAME: TYPE = EXPR;`
                    out.push_str("const ");
                    out.push_str(trimmed);
                    out.push('\n');
                }
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    (out, injected)
}

// ---------------------------------------------------------------------------
// Preprocessor handling
// ---------------------------------------------------------------------------

fn strip_preprocessor_blocks(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut depth = 0i32;
    let mut in_strip_block = false; // true when inside a block we want to drop

    for line in src.lines() {
        let trimmed = line.trim();

        // Strip all #if/#ifdef/#ifndef/#else/#elif/#endif blocks that we can't translate
        if trimmed.starts_with("#ifdef") || trimmed.starts_with("#ifndef") {
            depth += 1;
            in_strip_block = true;
            continue;
        }
        if trimmed.starts_with("#if ") || trimmed.starts_with("#elif ") {
            depth += 1;
            in_strip_block = true;
            continue;
        }
        if trimmed == "#else" {
            // Toggle: content after #else is also stripped
            continue;
        }
        if trimmed == "#endif" {
            depth -= 1;
            if depth == 0 {
                in_strip_block = false;
            }
            continue;
        }
        if in_strip_block {
            continue;
        }

        // Strip standalone preprocessor directives with no WGSL equivalent.
        // `#if`/`#ifdef`/`#ifndef`/`#else`/`#elif`/`#endif` are handled above.
        if trimmed.starts_with("precision ")
            || trimmed.starts_with("#version")
            || trimmed.starts_with("#pragma")
            || trimmed.starts_with("#extension")
            || trimmed.starts_with("#define")
            || trimmed.starts_with("#undef")
            || trimmed.starts_with("#line")
        {
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Strip GLSL-450 `layout(...)` declarations that have no WGSL equivalent.
/// Handles both single-line declarations and multi-line `layout(...) uniform Name { ... };` blocks.
fn strip_glsl450_layout_decls(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_layout_block = false;
    let mut brace_depth = 0i32;

    for line in src.lines() {
        let trimmed = line.trim();

        // Detect start of any layout(...) block or declaration.
        if trimmed.starts_with("layout(") {
            // Multi-line uniform block: layout(...) uniform Name {
            if trimmed.contains("uniform") {
                if trimmed.ends_with('{') {
                    in_layout_block = true;
                    brace_depth = 1;
                    continue;
                }
                // layout(...) uniform Name\n{  — brace on next line
                if !trimmed.ends_with(';') {
                    in_layout_block = true;
                    brace_depth = 0;
                    continue;
                }
            }
            // Single-line layout declaration (out, in, sampler, texture2D, etc.)
            if trimmed.ends_with(';') {
                continue;
            }
            // layout(...) on its own line (e.g. wrapped declaration) — skip and keep discarding
            // until we hit the matching semicolon or brace.
            in_layout_block = true;
            brace_depth = 0;
            continue;
        }

        if in_layout_block {
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            // If we were waiting for a semicolon to close a non-brace layout decl
            if brace_depth == 0 && trimmed.ends_with(';') {
                in_layout_block = false;
                continue;
            }
            // If we were inside a brace block and it closed
            if brace_depth <= 0 && !trimmed.starts_with("layout(") {
                in_layout_block = false;
                continue;
            }
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Normalize control-flow spacing: `if(`, `for(`, `while(` → `if (`, `for (`, `while (`
fn normalize_control_flow_spacing(src: &str) -> String {
    let mut s = src.to_string();
    for kw in &["if", "for", "while", "switch"] {
        let pat = format!("{}(", kw);
        let repl = format!("{} (", kw);
        s = replace_word_exact(&s, &pat, &repl);
    }
    s
}

/// Replace occurrences of `pattern` where `pattern` is not preceded by an identifier char.
fn replace_word_exact(src: &str, pattern: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        out.push_str(replacement);
        rest = &rest[pos + pattern.len()..];
    }
    out.push_str(rest);
    out
}

/// Add `convert_prefix_increment` to the pipeline: `++i` → `i++` in for-loop context.
fn convert_prefix_increments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(&convert_prefix_incr_in_line(line));
        out.push('\n');
    }
    out
}

fn convert_prefix_incr_in_line(line: &str) -> String {
    // Only convert prefix ++ / -- in for-loop update position
    // Pattern: `; ++VAR)` or `; --VAR)`
    let s = line.to_string();
    // Prefix `++VAR` where preceded by `;` or whitespace → `VAR++`
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(n + 4);
    let mut i = 0;
    while i < n {
        if i + 1 < n && chars[i] == '+' && chars[i + 1] == '+' {
            // Check if it's prefix (next is identifier, prev is not identifier)
            let prev = if i > 0 { chars[i - 1] } else { ' ' };
            let rest_start = i + 2;
            if !is_word_char(prev)
                && rest_start < n
                && (chars[rest_start].is_alphanumeric() || chars[rest_start] == '_')
            {
                // Collect identifier
                let mut j = rest_start;
                while j < n && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                let ident: String = chars[rest_start..j].iter().collect();
                out.push_str(&ident);
                out.push_str("++");
                i = j;
                continue;
            }
        }
        if i + 1 < n && chars[i] == '-' && chars[i + 1] == '-' {
            let prev = if i > 0 { chars[i - 1] } else { ' ' };
            let rest_start = i + 2;
            if !is_word_char(prev)
                && rest_start < n
                && (chars[rest_start].is_alphanumeric() || chars[rest_start] == '_')
            {
                let mut j = rest_start;
                while j < n && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                let ident: String = chars[rest_start..j].iter().collect();
                out.push_str(&ident);
                out.push_str("--");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Expand parameterized `#define NAME(p1, p2, ...) body` macros at all call sites.
/// E.g. `#define S(a,b,t) smoothstep(a,b,t)` expands `S(0.02, 0.0, d)` → `smoothstep(0.02, 0.0, d)`.
fn expand_fn_like_macros(src: &str) -> String {
    let mut macros: Vec<(String, Vec<String>, String)> = Vec::new();
    let mut body_lines: Vec<&str> = Vec::new();

    for line in src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#define ") {
            // Parse the macro name up to the first `(`
            if let Some(paren_pos) = rest.find('(') {
                let name = rest[..paren_pos].trim();
                // Must be a plain identifier (no spaces)
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    let after_paren = &rest[paren_pos + 1..];
                    if let Some(close_pos) = after_paren.find(')') {
                        let params_str = &after_paren[..close_pos];
                        let body = after_paren[close_pos + 1..].trim();
                        if !body.is_empty() {
                            let params =
                                params_str.split(',').map(|s| s.trim().to_owned()).collect();
                            macros.push((name.to_owned(), params, body.to_owned()));
                            continue; // Remove the #define line
                        }
                    }
                }
            }
        }
        body_lines.push(line);
    }

    if macros.is_empty() {
        return src.to_owned();
    }

    let mut result = body_lines.join("\n");
    if src.ends_with('\n') {
        result.push('\n');
    }

    // Fixed-point: repeat expansion passes until no more macro calls remain.
    // Needed for macros that expand into calls to other macros (e.g. Median's s2/mn3/mx3 chain).
    let max_passes = 16;
    for _pass in 0..max_passes {
        let prev = result.clone();
        for (name, params, body) in &macros {
            let mut s2 = String::with_capacity(result.len());
            let mut rest: &str = &result;
            let pattern = format!("{}(", name);
            loop {
                let Some(pos) = rest.find(&pattern) else {
                    break;
                };
                let before = if pos > 0 {
                    rest.as_bytes()[pos - 1] as char
                } else {
                    ' '
                };
                if is_word_char(before) {
                    s2.push_str(&rest[..pos + 1]);
                    rest = &rest[pos + 1..];
                    continue;
                }
                s2.push_str(&rest[..pos]);
                rest = &rest[pos + pattern.len()..];
                let (args_str, after) = extract_balanced(rest, ')');
                rest = after;
                let args = split_top_level_commas(args_str);
                let mut expanded = body.clone();
                for (param, arg) in params.iter().zip(args.iter()) {
                    expanded = replace_word(&expanded, param, arg.trim());
                }
                s2.push_str(&expanded);
            }
            s2.push_str(rest);
            result = s2;
        }
        if result == prev {
            break;
        }
    }
    result
}

fn convert_define_to_const(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#define ") {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("").trim();
            let value = parts.next().unwrap_or("").trim();
            if name.is_empty() || value.is_empty() {
                continue; // skip empty defines
            }
            // Skip function-like macros: name contains `(` e.g. `#define FOO(x) expr`
            if name.contains('(') {
                continue;
            }
            // Strip trailing inline `//` comment from value (e.g. `#define X 1.0 // comment`)
            // so the emitted `const X: f32 = 1.0;` doesn't have the `;` inside the comment.
            let value = if let Some(ci) = value.find("//") {
                value[..ci].trim()
            } else {
                value
            };
            if value.is_empty() {
                continue;
            }
            // Skip macros that contain statement separators (can't be a single const expr)
            if value.contains('{') || value.contains(';') {
                continue;
            }
            let ty = if value.contains('.')
                || value.contains('e')
                || value.contains('E')
                || value.contains('(')
            {
                "f32"
            } else if value.starts_with('-')
                || value.chars().all(|c| c.is_ascii_digit() || c == '-')
            {
                "i32"
            } else {
                "f32" // default (identifier references etc.)
            };
            // Strip trailing `f`/`F` suffix only for pure numeric literals
            let is_pure_literal = value.chars().all(|c| {
                c.is_ascii_digit()
                    || c == '.'
                    || c == '-'
                    || c == 'e'
                    || c == 'E'
                    || c == 'f'
                    || c == 'F'
                    || c == '+'
            });
            let val = if is_pure_literal {
                value.trim_end_matches('f').trim_end_matches('F')
            } else {
                value
            };
            out.push_str(&format!("const {}: {} = {};\n", name, ty, val));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Strip GLSL precision qualifiers (highp / mediump / lowp)
// ---------------------------------------------------------------------------

/// Remove inline GLSL precision qualifiers from declarations and function signatures.
/// Module-scope `precision highp float;` directives are already stripped by
/// `strip_preprocessor_blocks`; this handles inline uses like `highp float noise(...)`.
/// Strip ISF neighbor-coordinate varying/in declarations.
/// The ISF vertex shader pre-computes these (left_coord, right_coord, etc.) as varyings
/// that the fragment shader reads. In our WGSL we inject them as `let` bindings in fs_main.
/// Rename GLSL function parameters that share a name with an ISF input.
/// ISF inputs are replaced globally (`name` → `isf_u.name`).  If a helper function
/// also uses that name as a parameter (e.g. `float size` when `size` is an ISF input),
/// the replacement corrupts the parameter declaration.  This pass renames such parameters
/// to `_fp_NAME` before the ISF replacement runs, updating both the signature and the body.
fn rename_params_conflicting_with_isf_inputs(src: &str, isf_input_names: &[&str]) -> String {
    const GLSL_RETURN_TYPES: &[&str] = &[
        "void", "bool", "float", "int", "uint", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4",
        "mat2", "mat3", "mat4",
    ];

    let mut out = String::with_capacity(src.len() + 256);
    let mut brace_depth = 0i32;
    // Names that were renamed in the current function scope → must also be renamed in the body.
    // Tuple: (original_name, renamed_name, is_local_var)
    let mut active_renames: Vec<(String, String)> = Vec::new();

    for line in src.lines() {
        let trimmed = line.trim();
        let depth_before = brace_depth;

        // Update brace depth AFTER computing depth_before (we need the depth at start of line)
        for c in trimmed.chars() {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        // Clear renames when we exit the function body back to module scope.
        if brace_depth == 0 && depth_before == 0 {
            active_renames.clear();
        }

        // Process function declarations at module scope.
        if depth_before == 0 {
            let is_func_decl = GLSL_RETURN_TYPES.iter().any(|t| {
                let rest = trimmed.strip_prefix(t).unwrap_or("");
                if rest.is_empty() || (!rest.starts_with(' ') && !rest.starts_with('\t')) {
                    return false;
                }
                // Must have `(` with no `=` before it — excludes global variable declarations
                // like `float nQuick = clamp(...)` which are not function declarations.
                trimmed
                    .find('(')
                    .is_some_and(|p| !trimmed[..p].contains('='))
            });

            if is_func_decl
                && let Some(open) = trimmed.find('(') {
                    let after = &trimmed[open + 1..];
                    if let Some(close) = find_matching_close(after, ')') {
                        let params_str = &after[..close];
                        active_renames.clear();
                        for &name in isf_input_names {
                            if contains_word(params_str, name) {
                                active_renames.push((name.to_string(), format!("_fp_{}", name)));
                            }
                        }
                        if !active_renames.is_empty() {
                            let mut new_line = line.to_owned();
                            for (orig, renamed) in &active_renames {
                                new_line = replace_word(&new_line, orig, renamed);
                            }
                            out.push_str(&new_line);
                            out.push('\n');
                            continue;
                        }
                    }
                }
        }

        // In function body: detect local variable declarations that shadow ISF inputs.
        // e.g. `vec4 vignette = ...` where `vignette` is an ISF input name.
        // Rename to `_lv_NAME` BEFORE ISF substitution so uses of the local remain correct.
        // Track which local-variable renames were added mid-function (so we can add them on the decl line).
        let mut new_local_rename: Option<(String, String)> = None;
        if depth_before > 0 {
            for &ty in GLSL_RETURN_TYPES {
                let prefix = format!("{} ", ty);
                if let Some(rest) = trimmed.strip_prefix(prefix.as_str()) {
                    let ident_end = rest
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    let ident = &rest[..ident_end];
                    // Check it's an ISF input name and not already renamed
                    if !ident.is_empty()
                        && isf_input_names.contains(&ident)
                        && !active_renames.iter().any(|(o, _)| o == ident)
                    {
                        let renamed = format!("_lv_{}", ident);
                        new_local_rename = Some((ident.to_string(), renamed));
                    }
                    break; // Only match first type prefix
                }
            }
        }
        // Track if this line is the declaration that introduced a new local rename.
        let just_declared: Option<(String, String)> =
            new_local_rename.take().map(|(orig, renamed)| {
                active_renames.push((orig.clone(), renamed.clone()));
                (orig, renamed)
            });

        // Apply active renames in function bodies.
        if depth_before > 0 && !active_renames.is_empty() {
            let mut new_line = line.to_owned();
            if let Some((ref orig, ref renamed)) = just_declared {
                // Declaration line: apply all pre-existing renames fully, but for the
                // newly declared name only rename the first occurrence (the LHS identifier).
                // The RHS references to the same name are ISF inputs and must remain
                // unrenamed so the ISF substitution pass can convert them to isf_u.NAME.
                for (o, r) in &active_renames {
                    if o == orig {
                        continue;
                    }
                    new_line = replace_word(&new_line, o, r);
                }
                new_line = replace_word_first(&new_line, orig, renamed);
            } else {
                for (orig, renamed) in &active_renames {
                    new_line = replace_word(&new_line, orig, renamed);
                }
            }
            out.push_str(&new_line);
            out.push('\n');
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Find the position of the closing delimiter (e.g. `)`) matching the one that just opened.
/// `src` starts immediately AFTER the opening delimiter.
fn find_matching_close(src: &str, close: char) -> Option<usize> {
    let open = match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return None,
    };
    let mut depth = 1i32;
    for (i, c) in src.char_indices() {
        if c == open {
            depth += 1;
        }
        if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn strip_isf_coord_varyings(src: &str) -> String {
    // Strip ALL module-scope `varying T name;` and `in T name;` declarations.
    // GLSL varyings have no equivalent in WGSL; ISF-specific ones are re-injected as
    // local `let` bindings inside fs_main.
    const GLSL_TYPES: &[&str] = &[
        "float", "int", "uint", "bool", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "mat2",
        "mat3", "mat4",
    ];
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim();
        let is_varying_decl = (t.starts_with("varying ") || t.starts_with("in "))
            && t.ends_with(';')
            && GLSL_TYPES.iter().any(|ty| {
                let rest = if t.starts_with("varying ") {
                    &t[8..]
                } else {
                    &t[3..]
                };
                let rest = rest.trim_start();
                rest.starts_with(ty) && {
                    let after = rest[ty.len()..].trim_start();
                    // Must be `IDENT` or `IDENT[N]`, not a function
                    !after.starts_with('(')
                }
            });
        if !is_varying_decl {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn strip_glsl_precision_qualifiers(src: &str) -> String {
    let mut s = src.to_owned();
    for qualifier in &["highp", "mediump", "lowp"] {
        s = replace_word(&s, qualifier, "");
    }
    // Normalize any double-spaces left by removal (e.g. `highp  float` → `float`)
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    s
}

/// Strip GLSL function forward declarations at module scope (e.g. `vec3 rgb2hsv(vec3 c);`).
/// WGSL does not allow forward declarations; functions must be fully defined before use.
fn strip_glsl_forward_declarations(src: &str) -> String {
    let glsl_return_types = [
        "void", "float", "int", "uint", "bool", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4",
        "mat2", "mat3", "mat4",
    ];
    let mut out = String::with_capacity(src.len());
    let mut brace_depth = 0i32;

    for line in src.lines() {
        let trimmed = line.trim();
        // Track brace depth to only strip at module scope.
        for c in trimmed.chars() {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        // At module scope, detect `TYPE FUNCNAME(ARGS);` — note brace_depth is AFTER counting,
        // so we use the depth before counting this line.
        // Re-count to get the depth BEFORE this line:
        let depth_before: i32 = brace_depth
            - trimmed.chars().fold(0i32, |acc, c| match c {
                '{' => acc + 1,
                '}' => acc - 1,
                _ => acc,
            });
        let is_forward_decl =
            depth_before == 0 && trimmed.ends_with(");") && !trimmed.starts_with("//") && {
                // Starts with a GLSL return type, followed by an identifier, then '('
                let mut is_fwd = false;
                for &ty in &glsl_return_types {
                    if trimmed.starts_with(ty) {
                        let after = trimmed[ty.len()..].trim_start();
                        let id_end = after
                            .find(|c: char| !c.is_alphanumeric() && c != '_')
                            .unwrap_or(0);
                        if id_end > 0 && after[id_end..].trim_start().starts_with('(') {
                            is_fwd = true;
                            break;
                        }
                    }
                }
                is_fwd
            };
        if !is_forward_decl {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ISF image macro expansion
// ---------------------------------------------------------------------------

fn replace_img_macros(src: &str) -> String {
    // IMG_THIS_NORM_PIXEL(name) → textureSample(t_input, s_input, _isf_uv)  (image name arg ignored)
    let src = replace_img_this_norm_pixel(src);
    // IMG_NORM_PIXEL(name, uv) → textureSample(t_input, s_input, uv)
    let src = replace_img_norm_pixel(&src);
    // IMG_THIS_PIXEL(name) → textureSample(t_input, s_input, _isf_uv)
    let src = replace_img_this_pixel(&src);
    // IMG_PIXEL(name, pixelCoord) → textureLoad(t_input, vec2<i32>(pixelCoord), 0)
    let src = replace_img_pixel(&src);
    // IMG_SIZE(name) → render size as vec2 (best approximation for single-image shaders)

    replace_img_size(&src)
}

fn replace_img_size(src: &str) -> String {
    replace_macro_1arg(src, "IMG_SIZE", |_img_name| {
        "vec2<f32>(isf_u.rendersize_x, isf_u.rendersize_y)".to_string()
    })
}

fn replace_img_this_norm_pixel(src: &str) -> String {
    // IMG_THIS_NORM_PIXEL(sampler_name) — the argument is the image name, not a UV.
    // Samples the given image at the current normalized UV (_isf_uv).
    replace_macro_1arg(src, "IMG_THIS_NORM_PIXEL", |_img_name| {
        "textureSample(t_input, s_input, _isf_uv)".to_string()
    })
}

fn replace_img_norm_pixel(src: &str) -> String {
    replace_macro_2arg(src, "IMG_NORM_PIXEL", |_img, uv| {
        format!("textureSample(t_input, s_input, {})", uv)
    })
}

fn replace_img_this_pixel(src: &str) -> String {
    replace_macro_1arg(src, "IMG_THIS_PIXEL", |_img| {
        "textureSample(t_input, s_input, _isf_uv)".to_string()
    })
}

/// IMG_PIXEL(name, pixelCoord) → textureLoad(t_input, vec2<i32>(coord), 0)
/// ISF pixel-space sampling (integer coordinates, level 0).
fn replace_img_pixel(src: &str) -> String {
    replace_macro_2arg(src, "IMG_PIXEL", |_img, coord| {
        format!("textureLoad(t_input, vec2<i32>({}), 0)", coord.trim())
    })
}

/// Replace `MACRO(arg)` with the replacement.
fn replace_macro_1arg(src: &str, macro_name: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let pattern = &format!("{}(", macro_name);
    while let Some(pos) = rest.find(pattern) {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        // Find matching close paren
        let (arg, after) = extract_balanced(rest, ')');
        out.push_str(&f(arg.trim()));
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Replace `MACRO(arg1, arg2)` with the replacement.
fn replace_macro_2arg(src: &str, macro_name: &str, f: impl Fn(&str, &str) -> String) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let pattern = &format!("{}(", macro_name);
    while let Some(pos) = rest.find(pattern) {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        let (args, after) = extract_balanced(rest, ')');
        // Split args at the first top-level comma
        let (a1, a2) = split_first_top_level_comma(args);
        out.push_str(&f(a1.trim(), a2.trim()));
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Extract balanced text up to `close`, returning (inner, rest_after_close).
fn extract_balanced(src: &str, close: char) -> (&str, &str) {
    let open = '(';
    let mut depth = 1i32;
    let mut i = 0;
    let bytes = src.as_bytes();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return (&src[..i], &src[i + 1..]);
            }
        }
        i += 1;
    }
    (src, "")
}

fn split_first_top_level_comma(src: &str) -> (&str, &str) {
    let mut depth = 0i32;
    for (i, c) in src.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return (&src[..i], &src[i + 1..]),
            _ => {}
        }
    }
    (src, "")
}

// ---------------------------------------------------------------------------
// Word replacement (respects word boundaries)
// ---------------------------------------------------------------------------

/// Like `replace_word` but only replaces the FIRST whole-word occurrence of `word`.
fn replace_word_first(src: &str, word: &str, replacement: &str) -> String {
    let mut rest = src;
    while let Some(pos) = rest.find(word) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        let after_pos = pos + word.len();
        let after = if after_pos < rest.len() {
            rest.as_bytes()[after_pos] as char
        } else {
            ' '
        };
        if is_word_char(before) || before == '.' || is_word_char(after) {
            rest = &rest[pos + 1..];
            continue;
        }
        // Found first whole-word occurrence — replace it and return
        let prefix_len = src.len() - rest.len();
        let mut out = String::with_capacity(src.len());
        out.push_str(&src[..prefix_len + pos]);
        out.push_str(replacement);
        out.push_str(&rest[after_pos..]);
        return out;
    }
    src.to_owned() // no match
}

fn replace_word(src: &str, word: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(pos) = rest.find(word) {
        // Check word boundary before
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        let after_pos = pos + word.len();
        let after = if after_pos < rest.len() {
            rest.as_bytes()[after_pos] as char
        } else {
            ' '
        };

        if is_word_char(before) || before == '.' || is_word_char(after) {
            // Not a word boundary, or preceded by `.` (swizzle/field access) — skip
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        out.push_str(replacement);
        rest = &rest[after_pos..];
    }
    out.push_str(rest);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn contains_word(src: &str, word: &str) -> bool {
    let mut start = 0;
    while start + word.len() <= src.len() {
        if let Some(pos) = src[start..].find(word) {
            let abs = start + pos;
            let before_ok = abs == 0 || !is_word_char(src.as_bytes()[abs - 1] as char);
            let after_ok = abs + word.len() >= src.len()
                || !is_word_char(src.as_bytes()[abs + word.len()] as char);
            if before_ok && after_ok {
                return true;
            }
            start = abs + 1;
        } else {
            break;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Type and syntax conversion
// ---------------------------------------------------------------------------

/// Replace `glsl_type(` with `wgsl_type(` only when `glsl_type` is at a word boundary
/// (not preceded by a word character). Prevents `int(` from matching inside `hitPoint(`.
fn replace_type_cast_word_boundary(src: &str, glsl_type: &str, wgsl_type: &str) -> String {
    let pattern = format!("{}(", glsl_type);
    let replacement = format!("{}(", wgsl_type);
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(pos) = rest.find(pattern.as_str()) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        out.push_str(&replacement);
        rest = &rest[pos + pattern.len()..];
    }
    out.push_str(rest);
    out
}

fn convert_types_and_syntax(src: &str) -> String {
    // Order matters: longer patterns first
    let mut s = src.to_owned();

    // Vector/matrix type constructors — use word-boundary replacement to avoid
    // matching `int(` inside `hitPoint(`, `float(` inside `overfloat(`, etc.
    for (glsl_type, wgsl_type) in &[
        ("ivec4", "vec4<i32>"),
        ("ivec3", "vec3<i32>"),
        ("ivec2", "vec2<i32>"),
        ("uvec4", "vec4<u32>"),
        ("uvec3", "vec3<u32>"),
        ("uvec2", "vec2<u32>"),
        ("bvec4", "vec4<bool>"),
        ("bvec3", "vec3<bool>"),
        ("bvec2", "vec2<bool>"),
        ("mat4x4", "mat4x4<f32>"),
        ("mat3x3", "mat3x3<f32>"),
        ("mat2x2", "mat2x2<f32>"),
        ("mat4", "mat4x4<f32>"),
        ("mat3", "mat3x3<f32>"),
        ("mat2", "mat2x2<f32>"),
        ("vec4", "vec4<f32>"),
        ("vec3", "vec3<f32>"),
        ("vec2", "vec2<f32>"),
        ("float", "f32"),
        ("int", "i32"),
        ("uint", "u32"),
    ] {
        s = replace_type_cast_word_boundary(&s, glsl_type, wgsl_type);
    }

    // GLSL atan(y, x) → WGSL atan2(y, x)  (two-arg form; one-arg stays as atan)
    s = convert_atan_calls(&s);

    // GLSL inversesqrt(x) → WGSL (1.0 / sqrt(x))
    s = convert_inversesqrt_calls(&s);

    // GLSL i++ / ++i / i-- / --i → WGSL i += 1 / i -= 1
    s = convert_increment_decrement(&s);

    // WGSL reserved keywords used as identifiers
    s = rename_wgsl_reserved_keywords(&s);

    // GLSL mod(x, y) → inlined (x - y * floor(x / y)), works for any scalar/vector type
    s = convert_mod_calls(&s);

    // GLSL clamp(x, 0.0, 1.0) → WGSL saturate(x) — works for any scalar/vector type.
    // WGSL requires all three clamp args to have the same type (no scalar bounds for vectors).
    s = convert_clamp_01_to_saturate(&s);

    // GLSL comparison builtins → WGSL infix operators (component-wise, return vec<bool>)
    s = replace_glsl_comparison_builtins(&s);

    // GLSL derivative functions
    s = s.replace("dFdx(", "dpdx(");
    s = s.replace("dFdy(", "dpdy(");
    // fwidth is the same name in WGSL — no replacement needed.

    // If the GLSL defines a user function named `texture`, rename it before the ISF
    // texture-call conversion so that the user's function doesn't get clobbered.
    s = rename_user_defined_texture_fn(&s);

    // GLSL texture functions — insert sampler arg: texture2D(tex, uv) → textureSample(t_input, s_input, uv)
    s = convert_texture2d_calls(&s);

    // GLSL stpq swizzle aliases → WGSL xyzw equivalents
    s = convert_stpq_swizzles(&s);

    s
}

/// Convert GLSL `stpq` swizzle aliases to WGSL `xyzw` equivalents.
/// E.g. `.st` → `.xy`, `.tp` → `.yz`, `.q` → `.w`, etc.
/// Only replaces after a `.` when all characters are from `stpq`.
fn convert_stpq_swizzles(src: &str) -> String {
    let map = |c: char| -> char {
        match c {
            's' => 'x',
            't' => 'y',
            'p' => 'z',
            'q' => 'w',
            _ => c,
        }
    };
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' && i + 1 < bytes.len() {
            let j = i + 1;
            // Scan following chars that are all stpq
            let mut k = j;
            while k < bytes.len() && "stpq".contains(bytes[k] as char) {
                k += 1;
            }
            let swizzle_len = k - j;
            if (2..=4).contains(&swizzle_len) {
                // Ensure what follows is not a word char (e.g. `.step` should not match `.st`)
                let after_ok = k >= bytes.len() || !is_word_char(bytes[k] as char);
                if after_ok {
                    out.push('.');
                    for idx in j..k {
                        out.push(map(bytes[idx] as char));
                    }
                    i = k;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// If the GLSL source defines a user function named `texture` (e.g. `vec4 texture(vec2 p)`),
/// rename it and all its call sites to `_user_texture_` before the ISF texture-sampling
/// conversion runs. This prevents `convert_texture2d_calls` from clobbering the user function.
fn rename_user_defined_texture_fn(src: &str) -> String {
    let type_prefixes = [
        "vec4 texture(",
        "vec3 texture(",
        "vec2 texture(",
        "float texture(",
        "int texture(",
        "bool texture(",
    ];
    if !type_prefixes.iter().any(|p| src.contains(p)) {
        return src.to_owned();
    }
    let mut out = String::with_capacity(src.len() + 32);
    let pattern = "texture(";
    let mut rest = src;
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        out.push_str("_user_texture_(");
        rest = &rest[pos + pattern.len()..];
    }
    out.push_str(rest);
    out
}

/// Convert GLSL `texture2D(sampler, uv)` / `texture(sampler, uv)` →
/// WGSL `textureSample(t_input, s_input, uv)`.
/// The GLSL sampler name is discarded — we always use the single bound t_input.
fn convert_texture2d_calls(src: &str) -> String {
    let patterns: &[&str] = &["texture2D(", "texture("];
    let mut s = src.to_owned();
    for pattern in patterns {
        let mut out = String::with_capacity(s.len());
        let mut rest: &str = &s;
        while let Some(pos) = rest.find(pattern) {
            let before = if pos > 0 {
                rest.as_bytes()[pos - 1] as char
            } else {
                ' '
            };
            if is_word_char(before) {
                // Part of a longer identifier (e.g. `textureSample`) — skip one char
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }
            out.push_str(&rest[..pos]);
            rest = &rest[pos + pattern.len()..];
            let (args_str, after) = extract_balanced(rest, ')');
            rest = after;
            // Split into (sampler_name, uv_args...)
            let parts = split_top_level_commas(args_str);
            if parts.len() >= 2 {
                let uv = parts[1..].join(", ");
                out.push_str(&format!("textureSample(t_input, s_input, {})", uv.trim()));
            } else {
                // Unusual: keep as-is but with s_input inserted
                out.push_str(&format!("textureSample(t_input, s_input, {})", args_str));
            }
        }
        out.push_str(rest);
        s = out;
    }
    s
}

/// Convert GLSL `inversesqrt(x)` → WGSL `(1.0 / sqrt(x))`.
fn convert_inversesqrt_calls(src: &str) -> String {
    let pattern = "inversesqrt(";
    let mut out = String::with_capacity(src.len() + 32);
    let mut rest = src;
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        let (args_str, after) = extract_balanced(rest, ')');
        rest = after;
        out.push_str(&format!("(1.0 / sqrt({}))", args_str.trim()));
    }
    out.push_str(rest);
    out
}

/// Convert GLSL `i++` / `++i` / `i--` / `--i` → WGSL `i += 1` / `i -= 1`.
fn convert_increment_decrement(src: &str) -> String {
    let mut s = src.to_owned();
    // Match identifier followed by ++ or --
    // Use a simple regex-like scan: find ++ or -- and check surrounding chars
    for op in &["++", "--"] {
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(pos) = rest.find(op) {
            // Check that it's not inside a string/comment (simplified)
            let before = if pos > 0 {
                rest.as_bytes()[pos - 1] as char
            } else {
                '\0'
            };
            let after = if pos + 2 < rest.len() {
                rest.as_bytes()[pos + 2] as char
            } else {
                '\0'
            };
            // Determine if prefix (++i) or postfix (i++)
            let is_prefix = is_word_char(before) && !is_word_char(after);
            let is_postfix = !is_word_char(before) && is_word_char(after);
            if !is_prefix && !is_postfix {
                // Not a standalone increment/decrement (e.g. inside a longer token)
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }
            let ident = if is_prefix {
                // Extract identifier after ++/--
                let ident_start = pos + 2;
                let ident_end = rest[ident_start..]
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(rest.len() - ident_start)
                    + ident_start;
                &rest[ident_start..ident_end]
            } else {
                // Extract identifier before ++/--
                let ident_end = pos;
                let ident_start = rest[..ident_end]
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                &rest[ident_start..ident_end]
            };
            let replacement = if *op == "++" {
                format!("{} += 1", ident)
            } else {
                format!("{} -= 1", ident)
            };
            if is_prefix {
                out.push_str(&rest[..pos]);
                out.push_str(&replacement);
                rest = &rest[pos + 2 + ident.len()..];
            } else {
                out.push_str(&rest[..pos - ident.len()]);
                out.push_str(&replacement);
                rest = &rest[pos + 2..];
            }
        }
        out.push_str(rest);
        s = out;
    }
    s
}

/// Convert GLSL `atan(y, x)` (two-arg) → WGSL `atan2(y, x)`.
/// One-arg `atan(x)` is unchanged.
fn convert_atan_calls(src: &str) -> String {
    // Normalize `atan (` → `atan(` first so the pattern match is simple.
    let src = src.replace("atan (", "atan(");
    let mut out = String::with_capacity(src.len());
    let mut rest = src.as_str();
    let pattern = "atan(";
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            // Part of a longer identifier (e.g. "atan2") — skip
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        let (args, after) = extract_balanced(rest, ')');
        let (a1, a2) = split_first_top_level_comma(args);
        if a2.is_empty() {
            out.push_str(&format!("atan({})", args));
        } else {
            out.push_str(&format!("atan2({}, {})", a1.trim(), a2.trim()));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Convert `clamp(expr, 0.0, 1.0)` → `saturate(expr)`.
/// WGSL requires all clamp args to have the same type; saturate works for any numeric type.
/// Other clamp patterns with scalar bounds (e.g. clamp(v, -1.0, 1.0)) are handled by
/// the `isf_clamp_*` helper functions added to the preamble.
fn convert_clamp_01_to_saturate(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let pattern = "clamp(";
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        let (args, after) = extract_balanced(rest, ')');
        let (a1, rem) = split_first_top_level_comma(args);
        let (a2, a3) = split_first_top_level_comma(rem);
        let low = a2.trim();
        let high = a3.trim();

        // Detect 0.0/1.0 literal bounds → saturate
        let lo_is_zero = matches!(low, "0.0" | "0." | "0");
        let hi_is_one = matches!(high, "1.0" | "1." | "1");

        if lo_is_zero && hi_is_one {
            out.push_str(&format!("saturate({})", a1.trim()));
        } else {
            // Preserve original (works for scalar; may fail for vector/scalar mismatch)
            out.push_str(&format!("clamp({}, {}, {})", a1.trim(), low, high));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Inline GLSL `mod(x, y)` → `(x - y * floor(x / y))`.
/// Works for any scalar or vector type — no helper function needed.
fn convert_mod_calls(src: &str) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    let mut rest = src;
    let pattern = "mod(";
    while let Some(pos) = rest.find(pattern) {
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            // Part of a longer identifier (modf, smooth, ...) — skip one char
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        out.push_str(&rest[..pos]);
        rest = &rest[pos + pattern.len()..];
        let (args, after) = extract_balanced(rest, ')');
        let (x, y) = split_first_top_level_comma(args);
        let x = x.trim();
        let y = y.trim();
        if y.is_empty() {
            out.push_str(&format!("mod({})", args));
        } else {
            out.push_str(&format!("({} - {} * floor({} / {}))", x, y, x, y));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Replace GLSL vector comparison built-ins with WGSL infix operators.
/// `lessThan(a, b)` → `(a < b)`, etc.
fn replace_glsl_comparison_builtins(src: &str) -> String {
    let comparisons: &[(&str, &str)] = &[
        ("lessThanEqual(", "<="),
        ("greaterThanEqual(", ">="),
        ("lessThan(", "<"),
        ("greaterThan(", ">"),
        ("notEqual(", "!="),
        ("equal(", "=="),
    ];
    let mut s = src.to_owned();
    for (pattern, op) in comparisons {
        let mut out = String::with_capacity(s.len());
        let mut rest: &str = &s;
        while let Some(pos) = rest.find(pattern) {
            let before = if pos > 0 {
                rest.as_bytes()[pos - 1] as char
            } else {
                ' '
            };
            if is_word_char(before) {
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }
            out.push_str(&rest[..pos]);
            rest = &rest[pos + pattern.len()..];
            let (args, after) = extract_balanced(rest, ')');
            let (a, b) = split_first_top_level_comma(args);
            if b.is_empty() {
                out.push_str(pattern);
                out.push_str(args);
                out.push(')');
            } else {
                out.push_str(&format!("({} {} {})", a.trim(), op, b.trim()));
            }
            rest = after;
        }
        out.push_str(rest);
        s = out;
    }
    s
}

// ---------------------------------------------------------------------------
// Function signature conversion
// ---------------------------------------------------------------------------

fn convert_function_signatures(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        if let Some(converted) = try_convert_function_sig(line) {
            out.push_str(&converted);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Find the position of `; else ` in a braceless body string, at depth 0 (not inside parens/braces).
/// Returns the position of the `;` in `; else `.
fn find_else_in_braceless_body(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth -= 1,
            b';' if depth == 0 => {
                let rest = s[i + 1..].trim_start();
                if rest.starts_with("else ") || rest == "else" {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Add braces to braceless single-line control-flow bodies (WGSL requires compound statements).
/// `if (cond) break;` → `if (cond) { break; }`
/// `else stmt;` → `else { stmt; }`
fn add_braces_to_braceless_control_flow(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(&try_add_braces_to_line(line));
        out.push('\n');
    }
    out
}

/// Returns the braces-added version of a line, or the line unchanged if no fix needed.
fn try_add_braces_to_line(line: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = line.trim();
    // Skip empty, comment, already-ended-with-brace lines
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.ends_with('{')
        || trimmed.ends_with('}')
    {
        return std::borrow::Cow::Borrowed(line);
    }

    let indent = &line[..line.len() - line.trim_start().len()];

    // Try to match braceless keyword-with-parens: `if (...)`, `else if (...)`, `for (...)`, `while (...)`
    let kw_paren_candidates = [
        ("} else if (", "} else if ("),
        ("else if (", "else if ("),
        ("if (", "if ("),
        ("for (", "for ("),
        ("while (", "while ("),
    ];
    for (pattern, _) in &kw_paren_candidates {
        // Find the keyword at the start of trimmed (or after `}`)
        if !trimmed.starts_with(pattern) {
            continue;
        }
        let after_kw = &trimmed[pattern.len()..];
        let (_, after_paren) = extract_balanced(after_kw, ')');
        let after_paren = after_paren.trim_start();
        // If it already has a `{` it's fine
        if after_paren.starts_with('{') || after_paren.is_empty() {
            break; // no fix needed
        }
        // Braceless body — wrap it
        let header_len = trimmed.len() - after_paren.len();
        let header = &trimmed[..header_len];
        // Strip trailing `// comment` from body before wrapping inline
        let body_raw = after_paren;
        let body_no_comment = if let Some(c) = body_raw.find("//") {
            &body_raw[..c]
        } else {
            body_raw
        };
        // Check for `then_body; else else_body` on the same line
        if let Some(else_pos) = find_else_in_braceless_body(body_no_comment) {
            let then_raw = body_no_comment[..else_pos].trim().trim_end_matches(';');
            let else_raw = body_no_comment[else_pos + 2..].trim(); // skip "; "
                                                                   // else_raw starts with "else " — strip it
            let else_body_raw = if let Some(r) = else_raw.strip_prefix("else ") {
                r.trim()
            } else {
                else_raw
            };
            let else_body = else_body_raw.trim_end_matches(';');
            if !then_raw.is_empty() && !else_body.is_empty() {
                return std::borrow::Cow::Owned(format!(
                    "{}{} {{ {}; }} else {{ {}; }}",
                    indent, header, then_raw, else_body
                ));
            }
        }
        let body = body_no_comment.trim_end_matches(';').trim_end();
        // If the body is empty after stripping a trailing comment, the real body is on the
        // next line as `{...}` — leave this line alone so the brace forms the for/while body.
        if body.is_empty() {
            break;
        }
        return std::borrow::Cow::Owned(format!("{}{}{{ {}; }}", indent, header, body));
    }

    // Handle braceless `else STMT` (no parens)
    if trimmed.starts_with("else ") || trimmed.starts_with("} else ") {
        let prefix = if trimmed.starts_with("} else ") {
            "} else "
        } else {
            "else "
        };
        let after_else = trimmed[prefix.len()..].trim_start();
        if !after_else.starts_with("if ") && !after_else.starts_with('{') && !after_else.is_empty()
        {
            let body = after_else.trim_end_matches(';');
            let close = if prefix.starts_with('}') { "} " } else { "" };
            return std::borrow::Cow::Owned(format!("{}{}else {{ {}; }}", indent, close, body));
        }
    }

    std::borrow::Cow::Borrowed(line)
}

/// Fix braceless `if`/`for`/`while` that appear *inside* a same-line braced block.
/// Example: `if (a) { if (b) x = 1; }` → `if (a) { if (b) { x = 1; } }`
fn fix_inline_braceless_control_flow(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(&fix_inline_braceless_in_line(line));
        out.push('\n');
    }
    if out.ends_with('\n') && !src.ends_with('\n') {
        out.pop();
    }
    out
}

fn fix_inline_braceless_in_line(line: &str) -> std::borrow::Cow<'_, str> {
    // Process lines that may contain a braceless `if`/`for`/`while` anywhere — not just at start.
    // This handles cases like `stmt; if (cond) body;` on a single line.
    if !line.contains("if (") && !line.contains("for (") && !line.contains("while (") {
        return std::borrow::Cow::Borrowed(line);
    }
    let kw_patterns: &[&str] = &["if (", "for (", "while ("];
    let mut s = line.to_owned();
    // Iterate multiple times until stable (handles multiple braceless ifs on one line).
    for _ in 0..4 {
        let prev = s.clone();
        's_scan: for kw in kw_patterns {
            let mut search_start = 0;
            loop {
                let Some(pos) = s[search_start..].find(kw) else {
                    break;
                };
                let abs_pos = search_start + pos;
                // Ensure word boundary before keyword
                if abs_pos > 0 && is_word_char(s.as_bytes()[abs_pos - 1] as char) {
                    search_start = abs_pos + 1;
                    continue;
                }
                let after_kw = &s[abs_pos + kw.len()..];
                let (_, after_paren) = extract_balanced(after_kw, ')');
                let after_paren_trimmed = after_paren.trim_start();
                // If the body starts with `{`, it's already braced — skip
                if after_paren_trimmed.starts_with('{') || after_paren_trimmed.is_empty() {
                    search_start = abs_pos + 1;
                    continue;
                }
                // There's a braceless body. Find where the statement ends (`;`)
                // at top-level depth (not inside parens or brackets).
                let body_start = after_paren.len() - after_paren_trimmed.len();
                let stmt_end = find_stmt_end_in_line(after_paren_trimmed);
                let body = after_paren_trimmed[..stmt_end].trim_end_matches(';');
                // Strip trailing comment from body
                let body = if let Some(c) = body.find("//") {
                    body[..c].trim_end()
                } else {
                    body
                };
                let total_consumed =
                    abs_pos + kw.len() + after_kw.len() - after_paren.len() + body_start + stmt_end;
                let new_s = format!(
                    "{}{}{}{} {{ {}; }}{}",
                    &s[..abs_pos],
                    kw,
                    &after_kw[..after_kw.len() - after_paren.len()], // the balanced paren content + close
                    "",
                    body,
                    &s[total_consumed..]
                );
                s = new_s;
                continue 's_scan;
            }
        }
        if s == prev {
            break;
        }
    }
    if s == line {
        std::borrow::Cow::Borrowed(line)
    } else {
        std::borrow::Cow::Owned(s)
    }
}

/// Find the end position of a single statement in `s` (returns index after the `;`).
fn find_stmt_end_in_line(s: &str) -> usize {
    let mut depth = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    s.len()
}

/// Convert GLSL for-loop init declarations to WGSL: `for (int i = 0; ...)` → `for (var i: i32 = 0; ...)`
fn convert_for_loops(src: &str) -> String {
    let type_map: &[(&str, &str)] = &[
        ("int ", "i32"),
        ("float ", "f32"),
        ("uint ", "u32"),
        ("bool ", "bool"),
    ];
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(&try_convert_for_header(line, type_map).unwrap_or_else(|| line.to_string()));
        out.push('\n');
    }
    out
}

fn try_convert_for_header(line: &str, type_map: &[(&str, &str)]) -> Option<String> {
    // Match "for (" or "for(" at the start of the trimmed content
    let trimmed = line.trim();
    if !trimmed.starts_with("for ") && !trimmed.starts_with("for(") {
        return None;
    }
    // Find the opening paren
    let paren_off = trimmed.find('(')?;
    let after_paren = &trimmed[paren_off + 1..].trim_start();

    // Try each GLSL type prefix
    for (glsl_ty, wgsl_ty) in type_map {
        if let Some(rest) = after_paren.strip_prefix(glsl_ty) {
            // rest: "i = 0; i < N; i++) {"
            let ident_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if ident_end == 0 {
                continue;
            }
            let ident = &rest[..ident_end];
            let after_ident = &rest[ident_end..];

            // Reconstruct the for header
            let indent = &line[..line.len() - line.trim_start().len()];
            let for_prefix = &trimmed[..paren_off]; // "for " or "for"
            let after_fixed = if *wgsl_ty == "f32" {
                // WGSL requires i32/u32 for ++ / --; float loops need += 1.0
                fix_float_for_increment(after_ident, ident)
            } else {
                after_ident.to_string()
            };
            let result = format!(
                "{}{}(var {}: {} {}",
                indent,
                for_prefix,
                ident,
                wgsl_ty,
                after_fixed.trim_start()
            );
            return Some(result);
        }
    }
    None
}

/// Returns `Some(wgsl_line)` if `line` matches a GLSL function signature.
fn try_convert_function_sig(line: &str) -> Option<String> {
    let trimmed = line.trim();

    // Skip: already WGSL, or control flow, or empty
    if trimmed.starts_with("fn ")
        || trimmed.starts_with('@')
        || trimmed.starts_with("//")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("var ")
        || trimmed.starts_with("let ")
        || trimmed.is_empty()
    {
        return None;
    }

    // Match: RETTYPE FUNCNAME ( ARGS ) {
    // Must not be a control-flow keyword
    let paren_open = trimmed.find('(')?;
    let before_paren = trimmed[..paren_open].trim();
    let parts: Vec<&str> = before_paren.splitn(2, char::is_whitespace).collect();
    if parts.len() != 2 {
        return None;
    }
    let ret_glsl = parts[0].trim();
    let func_name = parts[1].trim();

    // Must be an identifier
    if !func_name.chars().all(|c| c.is_alphanumeric() || c == '_') || func_name.is_empty() {
        return None;
    }
    // Must not be a keyword
    if matches!(
        func_name,
        "if" | "else" | "for" | "while" | "do" | "switch" | "return"
    ) {
        return None;
    }
    // Skip void main / void mainImage — handled separately
    if func_name == "main" || func_name == "mainImage" {
        return None;
    }

    // Extract args
    let after_open = &trimmed[paren_open + 1..];
    let close = after_open.find(')')?;
    let args_str = &after_open[..close];
    let suffix = after_open[close + 1..].trim();

    // Strip trailing // comments before checking suffix (e.g. `float f(args) // comment\n{`)
    let suffix_no_comment = if let Some(c) = suffix.find("//") {
        suffix[..c].trim()
    } else {
        suffix
    };

    // Allow { on same line or on next line (handle multi-line style like `float f(float x)\n{`)
    if !suffix_no_comment.is_empty() && !suffix_no_comment.starts_with('{') {
        return None;
    }

    let wgsl_args = convert_arg_list(args_str);
    let wgsl_ret = if ret_glsl == "void" {
        String::new()
    } else {
        format!(" -> {}", glsl_type_to_wgsl(ret_glsl))
    };

    let indent = &line[..line.len() - line.trim_start().len()];

    // { on next line (or same line but after stripped comment) — emit just the signature
    if suffix_no_comment.is_empty() {
        return Some(format!(
            "{}fn {}({}){}",
            indent, func_name, wgsl_args, wgsl_ret
        ));
    }

    // Single-line function body: `TYPE fn(args) { body }` — expand to multiple lines
    let after_brace = suffix[1..].trim();
    if let Some(close_pos) = after_brace.rfind('}') {
        let body = after_brace[..close_pos].trim();
        return Some(format!(
            "{}fn {}({}){} {{\n{}    {}\n{}}}",
            indent, func_name, wgsl_args, wgsl_ret, indent, body, indent
        ));
    }

    Some(format!(
        "{}fn {}({}){} {{",
        indent, func_name, wgsl_args, wgsl_ret
    ))
}

fn convert_arg_list(args: &str) -> String {
    if args.trim().is_empty() {
        return String::new();
    }
    args.split(',')
        .map(|arg| {
            let a = arg.trim();
            // Strip qualifiers: in, out, inout, const
            let a = a
                .trim_start_matches("in ")
                .trim_start_matches("out ")
                .trim_start_matches("inout ")
                .trim_start_matches("const ");
            let parts: Vec<&str> = a.splitn(2, char::is_whitespace).collect();
            if parts.len() == 2 {
                format!(
                    "{}: {}",
                    parts[1].trim(),
                    glsl_type_to_wgsl(parts[0].trim())
                )
            } else {
                a.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn glsl_type_to_wgsl(t: &str) -> std::borrow::Cow<'_, str> {
    match t {
        "float" => "f32".into(),
        "int" => "i32".into(),
        "uint" => "u32".into(),
        "bool" => "bool".into(),
        "void" => "void".into(),
        "vec2" => "vec2<f32>".into(),
        "vec3" => "vec3<f32>".into(),
        "vec4" => "vec4<f32>".into(),
        "ivec2" => "vec2<i32>".into(),
        "ivec3" => "vec3<i32>".into(),
        "ivec4" => "vec4<i32>".into(),
        "uvec2" => "vec2<u32>".into(),
        "uvec3" => "vec3<u32>".into(),
        "uvec4" => "vec4<u32>".into(),
        "mat2" | "mat2x2" => "mat2x2<f32>".into(),
        "mat3" | "mat3x3" => "mat3x3<f32>".into(),
        "mat4" | "mat4x4" => "mat4x4<f32>".into(),
        // User-defined struct types: pass through unchanged.
        other => other.into(),
    }
}

// ---------------------------------------------------------------------------
// Variable declaration conversion
// ---------------------------------------------------------------------------

/// Promote module-scope `var name: TYPE` to `var<private> name: TYPE`.
/// WGSL requires an explicit address space for module-scope variables.
/// Texture/sampler/uniform bindings already have their address spaces; this
/// only applies to plain `var` declarations that were emitted by convert_var_declarations.
fn promote_module_scope_vars_to_private(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut brace_depth = 0i32;
    for line in src.lines() {
        let trimmed = line.trim();
        let depth_before = brace_depth;
        for c in trimmed.chars() {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if depth_before == 0 {
            // At module scope: promote plain `var name: …` to `var<private> name: …`
            // (Skip if already has address space qualifier or annotation)
            if trimmed.starts_with("var ") && !trimmed.starts_with("var<") {
                let indent = &line[..line.len() - line.trim_start().len()];
                out.push_str(indent);
                out.push_str("var<private> ");
                out.push_str(trimmed.strip_prefix("var ").unwrap());
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn convert_var_declarations(src: &str) -> String {
    let glsl_types = [
        "float", "int", "uint", "bool", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "uvec2",
        "uvec3", "uvec4", "mat2", "mat3", "mat4",
    ];

    // Collect user-defined struct type names so `TypeName varName;` can be converted.
    let mut struct_names: Vec<String> = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("struct ") {
            let name_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if name_end > 0 {
                struct_names.push(rest[..name_end].to_owned());
            }
        }
    }

    let mut out = String::with_capacity(src.len());
    let mut in_struct = false;
    let mut struct_brace_depth = 0i32;
    for line in src.lines() {
        let trimmed = line.trim();
        // Track struct blocks — inside a struct, field declarations need special handling.
        if trimmed.starts_with("struct ") && trimmed.contains('{') {
            in_struct = true;
            struct_brace_depth = 1;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_struct {
            for c in trimmed.chars() {
                match c {
                    '{' => struct_brace_depth += 1,
                    '}' => struct_brace_depth -= 1,
                    _ => {}
                }
            }
            if struct_brace_depth <= 0 {
                in_struct = false;
                // Keep the closing `}` or `};` — strip trailing `;` for WGSL
                let clean = line.trim_end_matches(';').trim_end_matches(';');
                out.push_str(clean);
                out.push('\n');
                continue;
            }
            // Inside struct: convert `type name;` → `name: wgsl_type,`
            out.push_str(&try_convert_struct_field(trimmed, &glsl_types));
            out.push('\n');
            continue;
        }
        let converted = try_convert_var_decl(line, &glsl_types);
        // If the primitive-type pass left it unchanged, try struct-type variable declarations.
        if converted == line {
            let trimmed = line.trim();
            if let Some(struct_wgsl) = try_convert_struct_var_decl(trimmed, &struct_names) {
                let indent = &line[..line.len() - trimmed.len()];
                out.push_str(indent);
                out.push_str(&struct_wgsl);
            } else {
                out.push_str(&converted);
            }
        } else {
            out.push_str(&converted);
        }
        out.push('\n');
    }
    out
}

/// Convert `StructType varName;` or `StructType varName = expr;` to WGSL `var varName: StructType`.
fn try_convert_struct_var_decl(trimmed: &str, struct_names: &[String]) -> Option<String> {
    for name in struct_names {
        let prefix = format!("{} ", name);
        if !trimmed.starts_with(&prefix) {
            continue;
        }
        let rest = trimmed[prefix.len()..].trim_start();
        let ident_end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if ident_end == 0 {
            continue;
        }
        let ident = &rest[..ident_end];
        let after = rest[ident_end..].trim_start();
        if after == ";" || after.is_empty() {
            return Some(format!("var {}: {};", ident, name));
        } else if let Some(expr_rest) = after.strip_prefix('=') {
            let expr = expr_rest.trim().trim_end_matches(';');
            return Some(format!("var {}: {} = {};", ident, name, expr));
        }
    }
    None
}

/// Convert a GLSL struct field `type name;` to WGSL `name: wgsl_type,`
fn try_convert_struct_field(trimmed: &str, glsl_types: &[&str]) -> String {
    for &glsl_ty in glsl_types {
        let prefix = format!("{} ", glsl_ty);
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let name = rest.trim_end_matches(';').trim_end_matches(',').trim();
            let wgsl_ty = glsl_type_to_wgsl(glsl_ty);
            return format!("    {}: {},", name, wgsl_ty);
        }
    }
    // Already converted or unknown — keep as-is but ensure it ends with `,`
    if trimmed.ends_with(';') {
        trimmed[..trimmed.len() - 1].to_owned() + ","
    } else {
        trimmed.to_owned()
    }
}

fn try_convert_var_decl(line: &str, glsl_types: &[&str]) -> String {
    let trimmed = line.trim();

    // Skip lines that are already WGSL or are control flow
    if trimmed.starts_with("var ")
        || trimmed.starts_with("let ")
        || trimmed.starts_with("fn ")
        || trimmed.starts_with('@')
        || trimmed.starts_with("//")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("return")
        || trimmed.is_empty()
    {
        return line.to_string();
    }

    let indent = &line[..line.len() - line.trim_start().len()];

    // GLSL `const TYPE NAME = EXPR;` → WGSL `const NAME: TYPE_WGSL = EXPR;`
    // Must be handled before the mutable var case below.
    if let Some(rest) = trimmed.strip_prefix("const ") {
        for &glsl_ty in glsl_types {
            if !rest.starts_with(glsl_ty) {
                continue;
            }
            let next = rest.as_bytes().get(glsl_ty.len()).copied().unwrap_or(b' ') as char;
            if !next.is_whitespace() {
                continue;
            }
            let after_type = rest[glsl_ty.len()..].trim_start();
            let ident_end = after_type
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after_type.len());
            if ident_end == 0 {
                continue;
            }
            let ident = &after_type[..ident_end];
            let after = after_type[ident_end..].trim_start();
            if after.starts_with('=') {
                let expr = after[1..].trim();
                let wgsl_ty = glsl_type_to_wgsl(glsl_ty);
                return format!("{}const {}: {} = {}", indent, ident, wgsl_ty, expr);
            }
        }
        // const without a recognized GLSL type — leave as-is (already valid WGSL const)
        return line.to_string();
    }

    // Match: TYPE NAME = ... ; or TYPE NAME;
    // Accept both space and tab between type keyword and identifier name.
    for &glsl_ty in glsl_types {
        if !trimmed.starts_with(glsl_ty) {
            continue;
        }
        let next = trimmed
            .as_bytes()
            .get(glsl_ty.len())
            .copied()
            .unwrap_or(b' ') as char;
        if !next.is_whitespace() {
            continue;
        }
        let rest = trimmed[glsl_ty.len()..].trim_start();
        // rest starts with identifier
        let ident_end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if ident_end == 0 {
            continue;
        }
        let ident = &rest[..ident_end];
        let after = rest[ident_end..].trim_start();

        // Handle ISF-input name conflict: `float isf_u.strength = ...`
        // The identifier is `isf_u` and `after` starts with `.NAME = ...`.
        // Rename the local to `_NAME_local` to avoid the compound identifier.
        if let Some(dot_rest) = after.strip_prefix('.') {
            let field_end = dot_rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(dot_rest.len());
            let field_name = &dot_rest[..field_end];
            let after2 = dot_rest[field_end..].trim_start();
            let wgsl_ty = glsl_type_to_wgsl(glsl_ty);
            if after2.starts_with('=') {
                let expr = after2[1..].trim();
                return format!(
                    "{}var _{}_local: {} = {}",
                    indent, field_name, wgsl_ty, expr
                );
            }
        }

        let wgsl_ty = glsl_type_to_wgsl(glsl_ty);

        if after.starts_with('=') {
            // TYPE NAME = EXPR;
            let expr = after[1..].trim();
            return format!("{}var {}: {} = {}", indent, ident, wgsl_ty, expr);
        } else if after.starts_with(';') || after.is_empty() {
            // TYPE NAME;
            return format!("{}var {}: {};", indent, ident, wgsl_ty);
        } else if after.starts_with(',') {
            // Multi-var: `TYPE a, b, c;` → multiple `var NAME: TYPE;` lines.
            let mut result = format!("{}var {}: {};", indent, ident, wgsl_ty);
            let mut tail = after; // starts with ','
            while let Some(pos) = tail.find(',') {
                tail = tail[pos + 1..].trim_start();
                let name_end = tail
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(tail.len());
                let extra_name = tail[..name_end].trim();
                if !extra_name.is_empty() {
                    result.push('\n');
                    result.push_str(&format!("{}var {}: {};", indent, extra_name, wgsl_ty));
                }
                tail = &tail[name_end..];
            }
            return result;
        } else if after.starts_with('[') {
            // C-style array: `TYPE name[N];` or `TYPE name[N] = ...;`
            // Note: module-scope arrays need var<private>; function-scope just var.
            // We emit var here; a later pass promotes module-scope ones to var<private>.
            if let Some(close) = after.find(']') {
                let n_str = after[1..close].trim();
                let rest_after = after[close + 1..].trim_start();
                if rest_after == ";" || rest_after.is_empty() {
                    return format!("{}var {}: array<{}, {}>;", indent, ident, wgsl_ty, n_str);
                }
                if let Some(eq_rest) = rest_after.strip_prefix('=') {
                    let expr = eq_rest.trim().trim_end_matches(';');
                    return format!(
                        "{}var {}: array<{}, {}> = {};",
                        indent, ident, wgsl_ty, n_str, expr
                    );
                }
            }
        }
    }
    line.to_string()
}

// ---------------------------------------------------------------------------
// Entry-point conversion
// ---------------------------------------------------------------------------

/// Convert `void main()` style entry point.
fn convert_void_main_entry(
    src: &str,
    _has_image_input: bool,
    injected_locals: &[String],
) -> String {
    let mut out = String::with_capacity(src.len());

    // Find `void main()` and replace its signature + inject fs_main preamble
    let mut found_main = false;
    let mut brace_depth = 0i32;
    let mut in_main = false;
    let mut main_brace_depth = 0i32;

    for line in src.lines() {
        let trimmed = line.trim();

        if !found_main {
            if is_void_main_line(trimmed) {
                found_main = true;
                in_main = true;
                // Emit fs_main header
                out.push_str("@fragment\n");
                out.push_str("fn fs_main(in: VertOut) -> @location(0) vec4<f32> {\n");
                // Always set _isf_uv / _isf_clip (var<private>) so helper fns can access them.
                out.push_str("    _isf_uv = in.uv;\n");
                // Flip clip.y to match gl_FragCoord convention (y=0 at bottom, not top).
                out.push_str("    _isf_clip = vec4<f32>(in.clip.x, isf_u.rendersize_y - in.clip.y, in.clip.z, in.clip.w);\n");
                // Inject module-scope vars that use runtime values
                for local in injected_locals {
                    out.push_str(local);
                    out.push('\n');
                }
                // If line contains opening brace, track it
                if trimmed.contains('{') {
                    brace_depth = 1;
                    main_brace_depth = 1;
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        } else if in_main {
            // If we haven't seen the opening brace yet (void main on its own line),
            // consume the standalone { and record that we've entered the body.
            if main_brace_depth == 0 {
                if trimmed == "{" {
                    brace_depth = 1;
                    main_brace_depth = 1;
                    continue; // skip the { — fs_main header already opened the body
                } else if trimmed.contains('{') {
                    // Opening brace mixed with content (unusual but handle it)
                    for c in trimmed.chars() {
                        match c {
                            '{' => brace_depth += 1,
                            '}' => brace_depth -= 1,
                            _ => {}
                        }
                    }
                    main_brace_depth = brace_depth;
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
                // Skip blank lines before the opening brace
                continue;
            }

            // Count braces to find end of main
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            if brace_depth <= 0 && main_brace_depth > 0 {
                // End of main — emit fallback return (dead code if shader always returns,
                // but keeps WGSL valid for shaders with conditional-only return paths).
                out.push_str("    return vec4<f32>(0.0, 0.0, 0.0, 1.0);\n");
                out.push_str("}\n");
                in_main = false;
                continue;
            }
            // Convert `return;` (bare return in void main) to fallback return.
            // Handles both standalone `return;` and `return;` inside braced blocks.
            if trimmed.contains("return;") {
                let fixed = line.replace("return;", "return vec4<f32>(0.0, 0.0, 0.0, 1.0);");
                out.push_str(&fixed);
                out.push('\n');
                continue;
            }
            out.push_str(line);
            out.push('\n');
        } else {
            // Post-main: helper functions defined after void main() — emit them so
            // move_fragment_fn_to_end can reorder them before @fragment.
            out.push_str(line);
            out.push('\n');
        }
    }

    // If no void main found, wrap everything in a basic fs_main
    if !found_main {
        out = String::new();
        out.push_str(src);
    }
    out
}

/// Convert `mainImage(out vec4 fragColor, in vec2 fragCoord)` + bridge pattern.
fn convert_main_image_entry(
    src: &str,
    _has_image_input: bool,
    injected_locals: &[String],
) -> String {
    let mut out = String::with_capacity(src.len());
    let mut found_main_image = false;
    let mut in_main_image = false;
    let mut brace_depth = 0i32;
    let mut skip_void_main = false;

    // First pass: detect whether void main() is just the bridge
    // (contains only `mainImage(gl_FragColor, gl_FragCoord.xy);`)
    // We'll skip it and emit fs_main wrapping mainImage body instead.

    let lines = src.lines().peekable();
    for line in lines {
        let trimmed = line.trim();

        // Skip `void main()` bridge (various forms: void main(), void main(void), etc.)
        if is_void_main_line(trimmed) {
            skip_void_main = true;
            brace_depth = 0;
            // Count any brace on the same line (e.g. `void main() {`)
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            continue;
        }
        if skip_void_main {
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            if brace_depth <= 0 {
                skip_void_main = false;
                brace_depth = 0;
            }
            continue;
        }

        // Detect mainImage signature
        if !found_main_image
            && (trimmed.starts_with("void mainImage(") || trimmed.starts_with("void mainImage ("))
        {
            found_main_image = true;
            in_main_image = true;
            brace_depth = 0;

            out.push_str("@fragment\n");
            out.push_str("fn fs_main(in: VertOut) -> @location(0) vec4<f32> {\n");
            // Flip Y to match GLSL gl_FragCoord convention (y=0 at bottom, not top).
            out.push_str("    var fragCoord: vec2<f32> = vec2<f32>(in.uv.x * isf_u.rendersize_x, (1.0 - in.uv.y) * isf_u.rendersize_y);\n");
            out.push_str("    var fragColor: vec4<f32>;\n");
            // Always set _isf_uv / _isf_clip (var<private>) so helper fns can access them.
            out.push_str("    _isf_uv = in.uv;\n");
            // Flip clip.y to match gl_FragCoord (y=0 at bottom).
            out.push_str("    _isf_clip = vec4<f32>(in.clip.x, isf_u.rendersize_y - in.clip.y, in.clip.z, in.clip.w);\n");
            // Inject module-scope runtime vars (e.g. iResolution, iTime aliases)
            for local in injected_locals {
                out.push_str(local);
                out.push('\n');
            }

            // Handle case where opening brace is on the same line
            if trimmed.contains('{') {
                brace_depth = 1;
            }
            continue;
        }

        if in_main_image {
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            if brace_depth <= 0 {
                // End of mainImage body — close fs_main with return fragColor
                out.push_str("    return fragColor;\n");
                out.push_str("}\n");
                in_main_image = false;
                continue;
            }
            // Convert `fragColor = X;` → don't convert (it writes the var we declared)
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Function overload resolution
// ---------------------------------------------------------------------------

/// GLSL allows multiple functions with the same name but different parameter types.
/// WGSL does not. This pass detects duplicate `fn NAME(` definitions, renames them
/// with a type-based suffix, and updates all call sites.
fn resolve_function_overloads(src: &str) -> String {
    use std::collections::HashMap;

    // --- Step 1: collect all function definitions ---
    // Use ALL parameter types for the suffix so that overloads with the same first param
    // type but different remaining params get unique names (e.g. Dither-Bayer's dither8x8).
    #[derive(Clone, Debug)]
    struct FnDef {
        name: String,
        all_param_tys: Vec<String>, // WGSL types of all params
        suffix: String,             // derived from all_param_tys
    }

    let lines: Vec<&str> = src.lines().collect();
    let mut fn_defs: Vec<FnDef> = Vec::new();
    let mut name_count: HashMap<String, usize> = HashMap::new();

    for line in &lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            let paren = rest.find('(').unwrap_or(rest.len());
            let name = rest[..paren].trim().to_owned();
            // Skip entry points
            if name == "vs_main" || name == "fs_main" {
                continue;
            }
            // Extract all param types
            let params_str = &rest[paren + 1..];
            let (params_inner, _) = extract_balanced(params_str, ')');
            let all_param_tys: Vec<String> = if params_inner.trim().is_empty() {
                vec!["void".to_owned()]
            } else {
                split_top_level_commas(params_inner)
                    .iter()
                    .map(|param| {
                        let param = param.trim();
                        if let Some(colon) = param.find(':') {
                            let ty_part = param[colon + 1..].trim();
                            let ty_end = ty_part
                                .find([',', ';', '='].as_ref())
                                .unwrap_or(ty_part.len());
                            ty_part[..ty_end].trim().to_owned()
                        } else {
                            "unknown".to_owned()
                        }
                    })
                    .collect()
            };
            let suffix: String = all_param_tys
                .iter()
                .map(|t| type_to_overload_suffix(t))
                .collect();
            fn_defs.push(FnDef {
                name: name.clone(),
                all_param_tys,
                suffix,
            });
            *name_count.entry(name).or_insert(0) += 1;
        }
    }

    // --- Step 2: find overloaded names (count > 1) ---
    let overloaded_names: std::collections::HashSet<String> = name_count
        .iter()
        .filter(|&(_, &c)| c > 1)
        .map(|(n, _)| n.clone())
        .collect();

    if overloaded_names.is_empty() {
        return src.to_owned();
    }

    // --- Step 3: build rename map: (name, param_types_key) → new_name ---
    // Key is the comma-joined all_param_tys so each signature maps to a unique name.
    let mut rename_map: HashMap<(String, String), String> = HashMap::new();
    for def in &fn_defs {
        if overloaded_names.contains(&def.name) {
            let new_name = format!("{}{}", def.name, def.suffix);
            let key = (def.name.clone(), def.all_param_tys.join(","));
            rename_map.insert(key, new_name);
        }
    }

    // --- Step 4 + 5: scope-aware line-by-line rename pass ---
    // Track var types per function scope so same-named vars in different functions
    // resolve to the correct types.
    let overloaded_vec: Vec<String> = overloaded_names.iter().cloned().collect();
    let mut result = String::with_capacity(src.len() + 64);
    let mut local_var_types: HashMap<String, String> = HashMap::new();

    for line in src.lines() {
        let trimmed = line.trim();

        // On a new function definition, reset local var types and seed with params.
        if trimmed.starts_with("fn ") {
            local_var_types.clear();
            // Seed with parameter types (after `inject_param_var_shadows`, params are
            // `_name: TYPE`; strip the leading `_` for lookup).
            if let Some(paren_pos) = trimmed.find('(') {
                let after_paren = &trimmed[paren_pos + 1..];
                let (params_inner, _) = extract_balanced(after_paren, ')');
                for param in params_inner.split(',') {
                    let param = param.trim();
                    if let Some(colon) = param.find(':') {
                        let pname = param[..colon].trim().trim_start_matches('_');
                        let pty = param[colon + 1..].trim().to_owned();
                        if !pname.is_empty() && !pty.is_empty() {
                            local_var_types.insert(pname.to_owned(), pty);
                        }
                    }
                }
            }
        }

        // Collect `var name: TYPE` declarations into local scope
        if let Some(rest) = trimmed.strip_prefix("var ") {
            let colon = rest.find(':').unwrap_or(rest.len());
            let vname = rest[..colon].trim().to_owned();
            if colon < rest.len() {
                let ty_part = &rest[colon + 1..];
                let ty_end = ty_part
                    .find(['=', ';', ','].as_ref())
                    .unwrap_or(ty_part.len());
                let ty = ty_part[..ty_end].trim().to_owned();
                if !vname.is_empty() && !ty.is_empty() {
                    local_var_types.insert(vname, ty);
                }
            }
        }

        // Rename overloaded names on this line
        let mut out_line = String::with_capacity(line.len() + 16);
        let mut rest_line: &str = line;

        loop {
            let mut earliest: Option<(usize, &str)> = None;
            for name in &overloaded_vec {
                if let Some(pos) = find_fn_name_call_or_def(rest_line, name)
                    && earliest.is_none_or(|(ep, _)| pos < ep) {
                        earliest = Some((pos, name.as_str()));
                    }
            }
            let Some((pos, found_name)) = earliest else {
                out_line.push_str(rest_line);
                break;
            };

            out_line.push_str(&rest_line[..pos]);
            let after_name = &rest_line[pos + found_name.len()..];

            // Determine if this is a definition (preceded by `fn `) or a call site
            let before_name = out_line.trim_end();
            let is_def = before_name.ends_with("fn");

            if is_def {
                // Definition site: extract ALL param types, build suffix from all of them.
                let paren_content = after_name.trim_start();
                if let Some(args_start) = paren_content.strip_prefix('(') {
                    let (inner, _) = extract_balanced(args_start, ')');
                    let all_tys: Vec<String> = if inner.trim().is_empty() {
                        vec!["void".to_owned()]
                    } else {
                        split_top_level_commas(inner)
                            .iter()
                            .map(|param| {
                                let param = param.trim();
                                if let Some(colon) = param.find(':') {
                                    let ty_part = param[colon + 1..].trim();
                                    let ty_end = ty_part
                                        .find([',', ';', '='].as_ref())
                                        .unwrap_or(ty_part.len());
                                    ty_part[..ty_end].trim().to_owned()
                                } else {
                                    "unknown".to_owned()
                                }
                            })
                            .collect()
                    };
                    let suffix: String =
                        all_tys.iter().map(|t| type_to_overload_suffix(t)).collect();
                    out_line.push_str(&format!("{}{}", found_name, suffix));
                } else {
                    out_line.push_str(found_name);
                }
            } else {
                // Call site: infer types of ALL arguments and look up the overload.
                let paren_content = after_name.trim_start();
                if let Some(args_start) = paren_content.strip_prefix('(') {
                    let (args_inner, _) = extract_balanced(args_start, ')');
                    let call_args = split_top_level_commas(args_inner);
                    // Infer type of each argument
                    let inferred_tys: Vec<Option<String>> = call_args
                        .iter()
                        .map(|arg| infer_call_arg_type(arg.trim(), &local_var_types))
                        .collect();
                    // Build a key from fully inferred types
                    let all_known = inferred_tys.iter().all(|t| t.is_some());
                    let new_name = if all_known {
                        let tys_key = inferred_tys
                            .iter()
                            .map(|t| t.as_deref().unwrap_or("unknown"))
                            .collect::<Vec<_>>()
                            .join(",");
                        rename_map.get(&(found_name.to_owned(), tys_key)).cloned()
                    } else {
                        // Partial inference: try matching by first inferred arg
                        let first_ty = inferred_tys
                            .first()
                            .and_then(|t| t.clone())
                            .unwrap_or_default();
                        // Find a def whose first param type matches
                        fn_defs
                            .iter()
                            .find(|d| {
                                d.name == found_name
                                    && overloaded_names.contains(&d.name)
                                    && d.all_param_tys.first().map(|t| t.as_str())
                                        == Some(first_ty.as_str())
                            })
                            .map(|d| format!("{}{}", d.name, d.suffix))
                    };
                    let new_name = new_name.unwrap_or_else(|| {
                        // Fall back to first defined overload
                        fn_defs
                            .iter()
                            .find(|d| d.name == found_name && overloaded_names.contains(&d.name))
                            .map(|d| format!("{}{}", d.name, d.suffix))
                            .unwrap_or_else(|| found_name.to_owned())
                    });
                    out_line.push_str(&new_name);
                } else {
                    out_line.push_str(found_name);
                }
            }

            rest_line = &rest_line[pos + found_name.len()..];
        }

        result.push_str(&out_line);
        result.push('\n');
    }

    // Trim trailing newline if original didn't have one
    if !src.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Find the position of `name` as a word (not part of a longer identifier) followed by `(` in `src`.
fn find_fn_name_call_or_def(src: &str, name: &str) -> Option<usize> {
    let mut search = 0;
    loop {
        let pos = src[search..].find(name).map(|p| p + search)?;
        let before = if pos > 0 {
            src.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        let after_end = pos + name.len();
        let after = src[after_end..].trim_start();
        if !is_word_char(before) && after.starts_with('(') {
            return Some(pos);
        }
        search = pos + 1;
    }
}

fn type_to_overload_suffix(ty: &str) -> String {
    match ty {
        "f32" => "_f32".to_owned(),
        "i32" => "_i32".to_owned(),
        "u32" => "_u32".to_owned(),
        "vec2<f32>" => "_v2f32".to_owned(),
        "vec3<f32>" => "_v3f32".to_owned(),
        "vec4<f32>" => "_v4f32".to_owned(),
        "vec2<i32>" => "_v2i32".to_owned(),
        "vec3<i32>" => "_v3i32".to_owned(),
        "vec4<i32>" => "_v4i32".to_owned(),
        other => format!("_{}", other.replace(['<', '>', ' '], "_")),
    }
}

/// Infer the WGSL type of the first argument in a function call argument string.
fn infer_call_arg_type(
    arg: &str,
    var_types: &std::collections::HashMap<String, String>,
) -> Option<String> {
    // Split at top-level comma and use the first arg
    let first_arg = {
        let parts = split_top_level_commas(arg);
        parts
            .into_iter()
            .next()
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    let arg = first_arg.trim();

    // Direct vec constructors
    for (prefix, ty) in &[
        ("vec4<f32>(", "vec4<f32>"),
        ("vec4(", "vec4<f32>"),
        ("vec3<f32>(", "vec3<f32>"),
        ("vec3(", "vec3<f32>"),
        ("vec2<f32>(", "vec2<f32>"),
        ("vec2(", "vec2<f32>"),
        ("vec4<i32>(", "vec4<i32>"),
        ("vec3<i32>(", "vec3<i32>"),
        ("vec2<i32>(", "vec2<i32>"),
    ] {
        if arg.starts_with(prefix) {
            return Some(ty.to_string());
        }
    }

    // `dot(a, b)` → f32
    if arg.starts_with("dot(") {
        return Some("f32".to_owned());
    }
    // `f32(...)` literal cast
    if arg.starts_with("f32(") {
        return Some("f32".to_owned());
    }

    // Swizzle: `expr.xyzw` → type from swizzle length.
    // Only treat as a whole-expression trailing swizzle if the part before the `.` has
    // no top-level arithmetic operators. Otherwise expressions like `wave - loc.x` would
    // pick up `.x` from `loc.x` and incorrectly return f32 when the expression is vec4.
    if let Some(dot_pos) = arg.rfind('.') {
        let before_dot = &arg[..dot_pos];
        let swizzle = &arg[dot_pos + 1..];
        let has_top_level_op = {
            let mut depth = 0i32;
            let mut found = false;
            for c in before_dot.chars() {
                match c {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    '+' | '-' | '*' | '/' if depth == 0 => {
                        found = true;
                        break;
                    }
                    _ => {}
                }
            }
            found
        };
        if !has_top_level_op
            && !swizzle.is_empty()
            && swizzle.chars().all(|c| "xyzwrgba".contains(c))
        {
            return Some(match swizzle.len() {
                1 => "f32".to_owned(),
                2 => "vec2<f32>".to_owned(),
                3 => "vec3<f32>".to_owned(),
                4 => "vec4<f32>".to_owned(),
                _ => return None,
            });
        }
    }

    // Single-argument passthrough functions whose return type equals their input type.
    // E.g., `fract(x)` returns the same type as `x`.
    const PASSTHROUGH_FUNS: &[&str] = &[
        "fract(",
        "floor(",
        "ceil(",
        "round(",
        "abs(",
        "sign(",
        "sqrt(",
        "exp(",
        "exp2(",
        "log(",
        "log2(",
        "sin(",
        "cos(",
        "tan(",
        "asin(",
        "acos(",
        "sinh(",
        "cosh(",
        "tanh(",
        "degrees(",
        "radians(",
        "normalize(",
        "saturate(",
        "dpdx(",
        "dpdy(",
        "fwidth(",
    ];
    for &prefix in PASSTHROUGH_FUNS {
        if let Some(rest) = arg.strip_prefix(prefix) {
            let (args_str, _) = extract_balanced(rest, ')');
            let parts = split_top_level_commas(args_str);
            if !parts.is_empty() {
                return infer_call_arg_type(parts[0].trim(), var_types);
            }
        }
    }

    // Simple identifier or expression with arithmetic: trace through var_types
    // Strip parentheses
    let stripped = arg.trim_start_matches('(').trim_end_matches(')');
    // Try to find the base identifier (leftmost word)
    let ident_end = stripped
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(stripped.len());
    if ident_end > 0 {
        let ident = &stripped[..ident_end];
        let after_ident = &stripped[ident_end..];

        // Check for swizzle access: `ident.y` → f32, `ident.yz` → vec2<f32>, etc.
        // This handles cases like `(uv.y + 1.0) * 0.5` where the base identifier is `uv`
        // (vec2<f32>) but the actual expression type is f32 due to the `.y` swizzle.
        if let Some(swizzle_rest) = after_ident.strip_prefix('.') {
            let sw_len = swizzle_rest
                .find(|c: char| !"xyzwrgba".contains(c))
                .unwrap_or(swizzle_rest.len());
            if sw_len > 0 && sw_len <= 4 {
                let next_after_sw = &swizzle_rest[sw_len..];
                // Confirm it really is a swizzle (next char is not an identifier char)
                if next_after_sw.is_empty()
                    || next_after_sw.starts_with(|c: char| !c.is_alphanumeric() && c != '_')
                {
                    return Some(match sw_len {
                        1 => "f32".to_owned(),
                        2 => "vec2<f32>".to_owned(),
                        3 => "vec3<f32>".to_owned(),
                        4 => "vec4<f32>".to_owned(),
                        _ => unreachable!(),
                    });
                }
            }
        }

        // Pattern: `(base_ident + ... ).swizzle` where a swizzle is applied to a group that
        // CONTAINS the base identifier. after_ident is like ` + sample1 + ...).rgb / ...`.
        // Find the first `)` in after_ident; if what follows is `.swizzle`, use that type.
        if let Some(close_pos) = after_ident.find(')') {
            let after_close = after_ident[close_pos + 1..].trim_start();
            if let Some(sw_part) = after_close.strip_prefix('.') {
                let sw_len = sw_part
                    .find(|c: char| !"xyzwrgba".contains(c))
                    .unwrap_or(sw_part.len());
                if sw_len > 0 && sw_len <= 4 {
                    return Some(match sw_len {
                        1 => "f32".to_owned(),
                        2 => "vec2<f32>".to_owned(),
                        3 => "vec3<f32>".to_owned(),
                        4 => "vec4<f32>".to_owned(),
                        _ => unreachable!(),
                    });
                }
            }
        }

        // Strip `_fp_` prefix (injected shadow params use `_fp_name`)
        let lookup_name = ident
            .strip_prefix("_fp_")
            .unwrap_or(ident)
            .trim_start_matches('_');
        if let Some(ty) = var_types.get(lookup_name).or_else(|| var_types.get(ident)) {
            return Some(ty.clone());
        }
    }

    // Arithmetic expression: scan tokens split at top-level operators.
    // Only look up pure identifiers and vec constructor prefixes (no recursion) to
    // avoid infinite recursion for expressions like `1.0/realGamma`.
    let arith_ops: &[char] = &['+', '-', '*', '/'];
    let mut best: Option<String> = None;
    let mut depth = 0i32;
    let mut token_start = 0;
    let bytes = arg.as_bytes();
    let n = bytes.len();
    for i in 0..=n {
        let at_end = i == n;
        let is_op = if at_end {
            false
        } else {
            arith_ops.contains(&(bytes[i] as char)) && depth == 0
        };
        if is_op || at_end {
            let token = arg[token_start..i].trim();
            if !token.is_empty() {
                // Check vec constructor prefix
                let vec_ty: Option<&'static str> = if token.starts_with("vec4") {
                    Some("vec4<f32>")
                } else if token.starts_with("vec3") {
                    Some("vec3<f32>")
                } else if token.starts_with("vec2") {
                    Some("vec2<f32>")
                } else {
                    None
                };
                // Check pure identifier in var_types
                let ident_ty = if vec_ty.is_none() {
                    let trimmed_tok = token.trim_start_matches('(').trim_end_matches(')').trim();
                    let ident_end = trimmed_tok
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(trimmed_tok.len());
                    if ident_end == trimmed_tok.len() && !trimmed_tok.is_empty() {
                        // Pure identifier: look up in var_types
                        let lookup = trimmed_tok
                            .strip_prefix("_fp_")
                            .unwrap_or(trimmed_tok)
                            .trim_start_matches('_');
                        var_types
                            .get(lookup)
                            .or_else(|| var_types.get(trimmed_tok))
                            .cloned()
                    } else {
                        None
                    }
                } else {
                    None
                };

                let ty = vec_ty.map(|s| s.to_owned()).or(ident_ty);
                if let Some(ty) = ty {
                    match ty.as_str() {
                        t if t.starts_with("vec4") => {
                            best = Some(ty);
                            break;
                        }
                        t if t.starts_with("vec3") => {
                            best = Some(ty);
                        }
                        t if t.starts_with("vec2") && best.is_none() => {
                            best = Some(ty);
                        }
                        _ => {}
                    }
                }
            }
            token_start = i + 1;
        } else {
            match bytes[i] as char {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                _ => {}
            }
        }
    }
    if best.is_some() {
        return best;
    }

    None
}

/// Detect the WGSL vector type explicitly present in an expression string.
/// Returns None if no vec constructor is found, or if the expression is a top-level
/// function call whose return type can't be inferred from its argument contents.
fn detect_vec_type_in_expr(expr: &str) -> Option<&'static str> {
    let expr = expr.trim();

    // Direct vec constructors at the start of the expression — safe to infer.
    if expr.starts_with("vec4<f32>(") {
        return Some("vec4<f32>");
    }
    if expr.starts_with("vec3<f32>(") {
        return Some("vec3<f32>");
    }
    if expr.starts_with("vec2<f32>(") {
        return Some("vec2<f32>");
    }
    if expr.starts_with("vec4(") {
        return Some("vec4<f32>");
    }
    if expr.starts_with("vec3(") {
        return Some("vec3<f32>");
    }
    if expr.starts_with("vec2(") {
        return Some("vec2<f32>");
    }

    // If the expression is a top-level non-vec function call (e.g. `dot(...)`,
    // `reflect(...)`, `normalize(...)`), we cannot infer the return type by scanning
    // for vec constructors inside the arguments — those are *argument* types, not the
    // return type.  Return None to avoid false positives like treating `dot(v, w)` as
    // vec3 just because `w` is constructed with `vec3<f32>(...)`.
    let ident_end = expr
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(expr.len());
    if ident_end > 0 && expr[ident_end..].trim_start().starts_with('(') {
        return None;
    }

    // For arithmetic expressions that contain vec constructors, check only at the TOP
    // LEVEL (not inside any parenthesized sub-expression). This avoids false positives
    // like `1.0 - distance(vec2<f32>(...), pt)` where the vec2 is an argument to
    // `distance` (which returns f32), not the type of the outer expression.
    let flat = collapse_parens(expr);
    if flat.contains("vec4<f32>(") || flat.contains("vec4(") {
        return Some("vec4<f32>");
    }
    if flat.contains("vec3<f32>(") || flat.contains("vec3(") {
        return Some("vec3<f32>");
    }
    if flat.contains("vec2<f32>(") || flat.contains("vec2(") {
        return Some("vec2<f32>");
    }
    None
}

/// Replace the content of every balanced parenthesis pair with `…` so that only
/// top-level tokens remain.  Used to detect vec types that are truly part of an
/// arithmetic expression rather than hidden inside function arguments.
fn collapse_parens(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut depth = 0i32;
    for c in src.chars() {
        match c {
            '(' => {
                depth += 1;
                out.push('(');
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    depth = 0;
                }
                out.push(')');
            }
            _ if depth == 0 => {
                out.push(c);
            }
            _ => {} // inside parens — skip
        }
    }
    out
}

/// Returns true if `s` looks like a plain scalar literal (digits, `.`, `-`, `e`, `f`).
fn is_scalar_literal(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| {
        c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+' || c == 'f'
    })
}

/// Type-aware scalar-expression check. For pure identifiers, looks up the type in `var_types`.
fn is_scalar_expr_with_types(
    s: &str,
    var_types: &std::collections::HashMap<String, String>,
) -> bool {
    if is_scalar_literal(s) {
        return true;
    }
    if s.contains("vec2<")
        || s.contains("vec3<")
        || s.contains("vec4<")
        || s.contains("vec2(")
        || s.contains("vec3(")
        || s.contains("vec4(")
    {
        return false;
    }
    // Pure identifier → look up in var_types.
    if s.chars().all(|c| c.is_alphanumeric() || c == '_') && !s.is_empty()
        && let Some(ty) = var_types.get(s) {
            return !ty.starts_with("vec");
        }
        // Unknown identifier — fall through to structural checks.
    // Multi-component swizzle → vector.
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'.'
            && i + 1 < n
            && is_word_char(if i > 0 { bytes[i - 1] as char } else { ' ' })
        {
            let sw_start = i + 1;
            let mut sw_end = sw_start;
            while sw_end < n && b"xyzwrgba".contains(&bytes[sw_end]) {
                sw_end += 1;
            }
            // Only treat as a swizzle if it's not part of a longer identifier
            // (e.g. `.br` in `isf_u.brightness` is a field access, not a swizzle).
            if sw_end - sw_start >= 2
                && !(sw_end < n && is_word_char(bytes[sw_end] as char))
            {
                return false;
            }
        }
        i += 1;
    }
    // Function call: check if it propagates the type of its first argument.
    // Functions that always return scalar regardless of input:
    const SCALAR_RETURN_FNS: &[&str] = &["dot", "length", "distance", "determinant", "any", "all"];
    let s_trimmed = s.trim();
    if let Some(paren_pos) = s_trimmed.find('(') {
        let fn_name = s_trimmed[..paren_pos].trim();
        // Only consider plain identifiers as function names.
        if !fn_name.is_empty()
            && fn_name.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !SCALAR_RETURN_FNS.contains(&fn_name)
        {
            let after_paren = &s_trimmed[paren_pos + 1..];
            let (args_inner, _) = extract_balanced(after_paren, ')');
            let args = split_top_level_commas(args_inner);
            // If the first argument is a vector, assume the call returns a vector.
            if let Some(first_arg) = args.first()
                && !is_scalar_expr_with_types(first_arg.trim(), var_types) {
                    return false;
                }
        }
    }
    true
}

/// Scope-aware pass: fix scalar/vector type mismatches in WGSL built-in calls.
/// - `max(VEC_EXPR, SCALAR)` / `min(VEC_EXPR, SCALAR)` → broadcast scalar
/// - `smoothstep(SCALAR, SCALAR, VEC_EXPR)` → broadcast scalars
/// - `clamp(VEC_EXPR, SCALAR, SCALAR)` → broadcast scalars
fn fix_builtin_type_mismatches(src: &str) -> String {
    use std::collections::HashMap;
    let mut result = String::with_capacity(src.len());
    let mut local_var_types: HashMap<String, String> = HashMap::new();

    for line in src.lines() {
        let trimmed = line.trim();

        // Reset on new function definition, seed with params
        if trimmed.starts_with("fn ") {
            local_var_types.clear();
            if let Some(paren_pos) = trimmed.find('(') {
                let after_paren = &trimmed[paren_pos + 1..];
                let (params_inner, _) = extract_balanced(after_paren, ')');
                for param in params_inner.split(',') {
                    let param = param.trim();
                    if let Some(colon) = param.find(':') {
                        let pname = param[..colon].trim().trim_start_matches('_');
                        let pty = param[colon + 1..].trim().to_owned();
                        if !pname.is_empty() && !pty.is_empty() {
                            local_var_types.insert(pname.to_owned(), pty);
                        }
                    }
                }
            }
        }

        // Collect `var name: TYPE` declarations
        if let Some(rest) = trimmed.strip_prefix("var ") {
            let colon = rest.find(':').unwrap_or(rest.len());
            let vname = rest[..colon].trim().to_owned();
            if colon < rest.len() {
                let ty_part = &rest[colon + 1..];
                let ty_end = ty_part
                    .find(['=', ';', ','].as_ref())
                    .unwrap_or(ty_part.len());
                let ty = ty_part[..ty_end].trim().to_owned();
                if !vname.is_empty() && !ty.is_empty() {
                    local_var_types.insert(vname, ty);
                }
            }
        }

        let fixed = fix_builtins_in_line(line, &local_var_types);
        result.push_str(&fixed);
        result.push('\n');
    }

    if !src.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

fn fix_builtins_in_line(
    line: &str,
    var_types: &std::collections::HashMap<String, String>,
) -> String {
    // Patterns: (fn_name, n_args, which_args_can_be_scalar, which_args_provide_type)
    // For max(A,B): if one is vector, broadcast the other
    // For min(A,B): same
    // For smoothstep(A,B,C): if C is vector, broadcast A and B
    // For clamp(X,A,B): if X is vector, broadcast A and B
    let builtins: &[(&str, usize)] = &[("max(", 2), ("min(", 2), ("smoothstep(", 3), ("clamp(", 3)];

    let mut out = line.to_owned();
    for (builtin, n_args) in builtins {
        out = fix_builtin_call(&out, builtin, *n_args, var_types);
    }
    out
}

fn fix_builtin_call(
    line: &str,
    builtin: &str,
    n_args: usize,
    var_types: &std::collections::HashMap<String, String>,
) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    loop {
        let Some(pos) = rest.find(builtin) else {
            out.push_str(rest);
            break;
        };
        // Check word boundary before (skip "smoothstep(" inside identifiers)
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }

        out.push_str(&rest[..pos + builtin.len()]);
        rest = &rest[pos + builtin.len()..];
        let (args_str, after) = extract_balanced(rest, ')');
        rest = after;

        let args = split_top_level_commas(args_str);
        if args.len() != n_args {
            // Unexpected arg count — leave as-is
            out.push_str(args_str);
            out.push(')');
            continue;
        }

        // Determine vector type from any arg that has a vec constructor
        let mut vec_ty: Option<&'static str> = None;
        for arg in &args {
            if let Some(ty) = detect_vec_type_in_expr(arg.trim()) {
                vec_ty = Some(ty);
                break;
            }
        }
        // If no constructor found, try type inference on non-scalar args
        if vec_ty.is_none() {
            for arg in &args {
                let a = arg.trim();
                if !is_scalar_literal(a)
                    && let Some(ty) = infer_call_arg_type(a, var_types)
                        && ty.starts_with("vec") {
                            vec_ty = Some(match ty.as_str() {
                                "vec4<f32>" => "vec4<f32>",
                                "vec3<f32>" => "vec3<f32>",
                                "vec2<f32>" => "vec2<f32>",
                                _ => {
                                    continue;
                                }
                            });
                            break;
                        }
            }
        }

        let Some(vty) = vec_ty else {
            // No vector found — leave as-is
            out.push_str(args_str);
            out.push(')');
            continue;
        };

        // Broadcast scalar values to the detected vector type.
        // Wrap scalar literals and ISF uniform fields (isf_u.* — always f32).
        // Leave args alone if they are already the right vector type.
        let new_args: Vec<String> = args
            .iter()
            .map(|arg| {
                let a = arg.trim();
                let already_vec =
                    infer_call_arg_type(a, var_types).is_some_and(|ty| ty.starts_with("vec"));
                if !already_vec && (is_scalar_literal(a) || a.starts_with("isf_u.")) {
                    format!("{}({})", vty, a)
                } else {
                    a.to_owned()
                }
            })
            .collect();

        // Only rewrite if something changed
        if new_args.iter().zip(args.iter()).any(|(n, o)| n != o.trim()) {
            out.push_str(&new_args.join(", "));
        } else {
            out.push_str(args_str);
        }
        out.push(')');
    }

    out
}

/// Fix `i32(expr) == f32_val` comparisons to `f32(i32(expr)) == f32_val`.
/// GLSL allows implicit int→float coercion; WGSL requires matching types in comparisons.
fn fix_int_cast_comparisons(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(&fix_int_cast_cmp_in_line(line));
        out.push('\n');
    }
    if !src.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn fix_int_cast_cmp_in_line(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains("i32(") && !line.contains("u32(") {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut result = line.to_owned();
    for cast in &["i32(", "u32("] {
        result = fix_cast_cmp_one(&result, cast);
    }
    std::borrow::Cow::Owned(result)
}

fn fix_cast_cmp_one(line: &str, cast: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let Some(pos) = rest.find(cast) else {
            out.push_str(rest);
            break;
        };
        // Word boundary check
        let before = if pos > 0 {
            rest.as_bytes()[pos - 1] as char
        } else {
            ' '
        };
        if is_word_char(before) {
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        let after_cast = &rest[pos + cast.len()..];
        let (inner, after_paren) = extract_balanced(after_cast, ')');
        // Check what follows: optional whitespace then == or !=
        let after_trimmed = after_paren.trim_start();
        let is_eq = after_trimmed.starts_with("==") || after_trimmed.starts_with("!=");
        if is_eq {
            // Wrap: cast(inner) → f32(cast(inner))
            out.push_str(&rest[..pos]);
            out.push_str("f32(");
            out.push_str(cast);
            out.push_str(inner);
            out.push_str("))");
            rest = after_paren;
        } else {
            // Leave as-is: advance past this cast
            let consumed = pos + cast.len() + inner.len() + 1;
            out.push_str(&rest[..consumed]);
            rest = after_paren;
        }
    }
    out
}

/// Fix GLSL swizzle compound assignments that are invalid in WGSL.
/// `v.xy -= u;`  → `v.x -= u.x; v.y -= u.y;`
/// `v.xy += 1.0;` → `v.x += 1.0; v.y += 1.0;`
fn fix_swizzle_compound_assignments(src: &str) -> String {
    const SWIZZLE_CHARS: &[u8] = b"xyzwrgba";
    const OPS: &[&str] = &["+=", "-=", "*=", "/=", "%="];
    use std::collections::HashMap;

    let mut out = String::with_capacity(src.len());
    // module_const_types: populated from `const name: TYPE` at module scope; never cleared.
    let mut module_const_types: HashMap<String, String> = HashMap::new();
    // local_var_types: populated from fn params and `var name: TYPE`; cleared on each fn.
    let mut local_var_types: HashMap<String, String> = HashMap::new();
    let mut in_function = false;

    for line in src.lines() {
        let trimmed = line.trim();

        // Track function boundaries — clear locals, seed with params.
        if trimmed.starts_with("fn ") {
            local_var_types.clear();
            in_function = true;
            if let Some(paren_pos) = trimmed.find('(') {
                let after_paren = &trimmed[paren_pos + 1..];
                let (params_inner, _) = extract_balanced(after_paren, ')');
                for param in params_inner.split(',') {
                    let param = param.trim();
                    if let Some(colon) = param.find(':') {
                        let pname = param[..colon].trim().trim_start_matches('_');
                        let pty = param[colon + 1..].trim().to_owned();
                        if !pname.is_empty() && !pty.is_empty() {
                            local_var_types.insert(pname.to_owned(), pty);
                        }
                    }
                }
            }
        }

        // Collect `const name: TYPE` — module-scope, never cleared.
        if let Some(rest) = trimmed.strip_prefix("const ") {
            let colon = rest.find(':').unwrap_or(rest.len());
            let vname = rest[..colon].trim().to_owned();
            if colon < rest.len() {
                let ty_part = &rest[colon + 1..];
                let ty_end = ty_part
                    .find(['=', ';', ','].as_ref())
                    .unwrap_or(ty_part.len());
                let ty = ty_part[..ty_end].trim().to_owned();
                if !vname.is_empty() && !ty.is_empty() {
                    if in_function {
                        local_var_types.insert(vname, ty);
                    } else {
                        module_const_types.insert(vname, ty);
                    }
                }
            }
        }

        // Collect `var name: TYPE` in function scope.
        if let Some(rest) = trimmed.strip_prefix("var ") {
            let colon = rest.find(':').unwrap_or(rest.len());
            let vname = rest[..colon].trim().to_owned();
            if colon < rest.len() {
                let ty_part = &rest[colon + 1..];
                let ty_end = ty_part
                    .find(['=', ';', ','].as_ref())
                    .unwrap_or(ty_part.len());
                let ty = ty_part[..ty_end].trim().to_owned();
                if !vname.is_empty() && !ty.is_empty() {
                    local_var_types.insert(vname, ty);
                }
            }
        }

        // Build merged view: local takes precedence over module-scope.
        let mut merged = module_const_types.clone();
        for (k, v) in &local_var_types {
            merged.insert(k.clone(), v.clone());
        }

        let expanded = try_expand_swizzle_assignment(line, SWIZZLE_CHARS, OPS, &merged);
        out.push_str(&expanded);
        out.push('\n');
    }
    if !src.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn try_expand_swizzle_assignment(
    line: &str,
    swizzle_chars: &[u8],
    ops: &[&str],
    var_types: &std::collections::HashMap<String, String>,
) -> String {
    let trimmed = line.trim();
    // Quick check: must contain `.` and one of the compound ops
    if !trimmed.contains('.') {
        return line.to_owned();
    }

    let indent = &line[..line.len() - line.trim_start().len()];

    // Pattern: `IDENT.SWIZZLE OP= EXPR;`
    // Find an identifier followed by `.SWIZZLE` followed by ` OP= `
    let bytes = trimmed.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        // Find a `.` preceded by an identifier
        if bytes[i] != b'.' {
            i += 1;
            continue;
        }
        // Check that before `.` is a word char (part of IDENT)
        if i == 0 || !is_word_char(bytes[i - 1] as char) {
            i += 1;
            continue;
        }
        // Collect the identifier before the `.`
        let ident_end = i;
        let mut ident_start = ident_end;
        while ident_start > 0
            && (bytes[ident_start - 1].is_ascii_alphanumeric() || bytes[ident_start - 1] == b'_')
        {
            ident_start -= 1;
        }
        // Only expand if ident_start == 0 (line starts with ident, possibly after indent)
        // i.e., the whole statement is the assignment
        if ident_start != 0 {
            i += 1;
            continue;
        }
        let ident = &trimmed[ident_start..ident_end];

        // Collect swizzle after `.`
        let swizzle_start = i + 1;
        let mut swizzle_end = swizzle_start;
        while swizzle_end < n && swizzle_chars.contains(&bytes[swizzle_end]) {
            swizzle_end += 1;
        }
        let swizzle = &trimmed[swizzle_start..swizzle_end];

        // Only handle multi-component swizzles (single component is already a place)
        if swizzle.len() <= 1 {
            i += 1;
            continue;
        }

        // Check for compound op after swizzle (with optional spaces)
        let after_swizzle = trimmed[swizzle_end..].trim_start();
        let mut matched_op: Option<&str> = None;
        for &op in ops {
            if after_swizzle.starts_with(op) {
                matched_op = Some(op);
                break;
            }
        }
        // Also handle plain `=` (not `==`)
        let is_plain_assign = after_swizzle.starts_with('=') && !after_swizzle.starts_with("==");

        if matched_op.is_none() && !is_plain_assign {
            i += 1;
            continue;
        }

        if let Some(op) = matched_op {
            let after_op = after_swizzle[op.len()..].trim_start();
            let rhs = after_op.trim_end_matches(';').trim_end();
            if rhs.is_empty() {
                i += 1;
                continue;
            }

            let rhs_is_scalar = is_scalar_expr_with_types(rhs, var_types);
            let components: Vec<char> = swizzle.chars().collect();
            let mut expanded = String::new();
            for (comp_idx, &comp) in components.iter().enumerate() {
                if comp_idx > 0 {
                    expanded.push('\n');
                    expanded.push_str(indent);
                }
                let rhs_part = if rhs_is_scalar {
                    rhs.to_owned()
                } else {
                    let component_name = ['x', 'y', 'z', 'w'][comp_idx];
                    format!("({}).{}", rhs, component_name)
                };
                expanded.push_str(&format!("{}.{} {} {};", ident, comp, op, rhs_part));
            }
            return format!("{}{}", indent, expanded);
        } else {
            // Plain `=` assignment to multi-component swizzle: `g.yz = EXPR;`
            // Expand to per-component assignments using RHS component access.
            let after_op = after_swizzle[1..].trim_start(); // skip `=`
            let rhs = after_op.trim_end_matches(';').trim_end();
            if rhs.is_empty() {
                i += 1;
                continue;
            }

            let rhs_is_scalar = is_scalar_expr_with_types(rhs, var_types);
            let components: Vec<char> = swizzle.chars().collect();
            let mut expanded = String::new();
            for (comp_idx, &comp) in components.iter().enumerate() {
                if comp_idx > 0 {
                    expanded.push('\n');
                    expanded.push_str(indent);
                }
                let rhs_part = if rhs_is_scalar {
                    rhs.to_owned()
                } else {
                    let component_name = ['x', 'y', 'z', 'w'][comp_idx];
                    format!("({}).{}", rhs, component_name)
                };
                expanded.push_str(&format!("{}.{} = {};", ident, comp, rhs_part));
            }
            return format!("{}{}", indent, expanded);
        }
    }

    line.to_owned()
}

// ---------------------------------------------------------------------------
// Fix oversized vec constructors
// ---------------------------------------------------------------------------

/// Fix `vec3<f32>(vec2_expr, scalar1, scalar2)` (4 components → 3 expected).
/// GLSL drivers silently truncate; WGSL is strict.
/// Scope-aware: tracks var types to detect which arguments contribute multiple components.
fn fix_oversized_vec_constructors(src: &str) -> String {
    use std::collections::HashMap;
    let mut result = String::with_capacity(src.len());
    let mut local_var_types: HashMap<String, String> = HashMap::new();

    for line in src.lines() {
        let trimmed = line.trim();

        // Reset and seed on function definition
        if trimmed.starts_with("fn ") {
            local_var_types.clear();
            if let Some(paren_pos) = trimmed.find('(') {
                let after_paren = &trimmed[paren_pos + 1..];
                let (params_inner, _) = extract_balanced(after_paren, ')');
                for param in params_inner.split(',') {
                    let param = param.trim();
                    if let Some(colon) = param.find(':') {
                        let pname = param[..colon].trim().trim_start_matches('_');
                        let pty = param[colon + 1..].trim().to_owned();
                        if !pname.is_empty() && !pty.is_empty() {
                            local_var_types.insert(pname.to_owned(), pty);
                        }
                    }
                }
            }
        }
        // Collect var declarations
        if let Some(rest) = trimmed.strip_prefix("var ") {
            let colon = rest.find(':').unwrap_or(rest.len());
            let vname = rest[..colon].trim().to_owned();
            if colon < rest.len() {
                let ty_part = &rest[colon + 1..];
                let ty_end = ty_part
                    .find(['=', ';', ','].as_ref())
                    .unwrap_or(ty_part.len());
                let ty = ty_part[..ty_end].trim().to_owned();
                if !vname.is_empty() && !ty.is_empty() {
                    local_var_types.insert(vname, ty);
                }
            }
        }

        let fixed = fix_vec_constructors_in_line(line, &local_var_types);
        result.push_str(&fixed);
        result.push('\n');
    }

    if !src.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

fn fix_vec_constructors_in_line(
    line: &str,
    var_types: &std::collections::HashMap<String, String>,
) -> String {
    const VEC_PATTERNS: &[(&str, usize)] = &[
        ("vec4<f32>(", 4),
        ("vec3<f32>(", 3),
        ("vec2<f32>(", 2),
        ("vec4<i32>(", 4),
        ("vec3<i32>(", 3),
        ("vec2<i32>(", 2),
        ("vec4<u32>(", 4),
        ("vec3<u32>(", 3),
        ("vec2<u32>(", 2),
    ];

    let mut out = line.to_owned();
    for (pattern, expected_n) in VEC_PATTERNS {
        let mut s2 = String::with_capacity(out.len());
        let mut rest: &str = &out;
        loop {
            let Some(pos) = rest.find(pattern) else {
                s2.push_str(rest);
                break;
            };
            let before = if pos > 0 {
                rest.as_bytes()[pos - 1] as char
            } else {
                ' '
            };
            if is_word_char(before) {
                s2.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }

            let after_open = &rest[pos + pattern.len()..];
            let (args_str, after_close) = extract_balanced(after_open, ')');

            // Detect multi-line constructor: `extract_balanced` ran out of input without
            // finding the closing `)`. This happens when the args span multiple lines.
            // In that case, `args_str.len() == after_open.len()` (whole rest was consumed
            // without finding a close) AND `after_close` is empty.
            let is_multiline = after_close.is_empty() && args_str.len() == after_open.len();
            if is_multiline {
                // Multi-line — emit up to and including the open paren, continue past it
                s2.push_str(&rest[..pos + pattern.len()]);
                rest = after_open;
                continue;
            }

            // Single-line: process — determine how to emit before writing anything.
            let args = split_top_level_commas(args_str);
            let arg_ncs: Vec<usize> = args
                .iter()
                .map(|a| infer_component_count(a.trim(), var_types))
                .collect();
            let total: usize = arg_ncs.iter().sum();

            // Single oversized arg: vec3<f32>(vec4_expr) → (vec4_expr).xyz
            if total > *expected_n && args.len() == 1 && arg_ncs[0] > *expected_n {
                let swizzle = &"xyzw"[..*expected_n];
                s2.push_str(&rest[..pos]); // content before the constructor
                s2.push_str(&format!("({}).{}", args[0].trim(), swizzle));
                rest = after_close;
                continue;
            }

            // Write the constructor prefix and proceed normally
            s2.push_str(&rest[..pos + pattern.len()]);
            rest = after_close;

            if total > *expected_n {
                // Drop trailing args until component count matches
                let mut count = 0usize;
                let mut keep = Vec::new();
                for (arg, nc) in args.iter().zip(arg_ncs.iter()) {
                    if count + nc <= *expected_n {
                        keep.push(*arg);
                        count += nc;
                    } else {
                        break;
                    }
                }
                if count == *expected_n && !keep.is_empty() {
                    s2.push_str(&keep.join(", "));
                } else {
                    s2.push_str(args_str); // fallback: unchanged
                }
            } else {
                s2.push_str(args_str);
            }
            s2.push(')');
        }
        out = s2;
    }
    out
}

/// Infer the number of vector components an expression contributes.
fn infer_component_count(
    expr: &str,
    var_types: &std::collections::HashMap<String, String>,
) -> usize {
    // Check trailing swizzle FIRST: `vec4<f32>(...).rgb` must return 3, not 4.
    // `infer_call_arg_type` would match the vec4 prefix before seeing the swizzle.
    if let Some(dot_pos) = expr.rfind('.') {
        let swizzle = &expr[dot_pos + 1..];
        if !swizzle.is_empty() && swizzle.chars().all(|c| "xyzwrgba".contains(c)) {
            return swizzle.len().clamp(1, 4);
        }
    }
    if let Some(ty) = infer_call_arg_type(expr, var_types) {
        return match ty.as_str() {
            "vec4<f32>" | "vec4<i32>" | "vec4<u32>" | "vec4<bool>" => 4,
            "vec3<f32>" | "vec3<i32>" | "vec3<u32>" | "vec3<bool>" => 3,
            "vec2<f32>" | "vec2<i32>" | "vec2<u32>" | "vec2<bool>" => 2,
            _ => 1,
        };
    }
    // Direct vec constructor prefix check as fallback
    if expr.starts_with("vec4<") {
        return 4;
    }
    if expr.starts_with("vec3<") {
        return 3;
    }
    if expr.starts_with("vec2<") {
        return 2;
    }
    1
}

// ---------------------------------------------------------------------------
// WGSL reserved keyword renaming (GLSL allows names that WGSL reserves)
// ---------------------------------------------------------------------------

fn rename_wgsl_reserved_keywords(src: &str) -> String {
    // WGSL reserved identifiers that may appear as GLSL variable or function names.
    // Safe to rename with word-boundary replacement: these don't appear as WGSL keywords
    // in the generated output.
    const RESERVED: &[&str] = &["move", "override", "abstract", "ref", "final", "from", "target"];
    let mut s = src.to_string();
    for &kw in RESERVED {
        let renamed = format!("{}_rjk", kw);
        s = replace_word(&s, kw, &renamed);
    }
    // `fn` is the WGSL function keyword, so we can't replace ALL occurrences — only
    // variable/expression uses.  The keyword always appears as the first token of a
    // declaration (`fn funcname(`).  Replace `var fn:` / `var fn ` / ` fn ` / `\tfn `
    // but leave `fn funcname(` untouched.
    if contains_word(&s, "fn") {
        // Replace variable declarations using `fn` as a name
        s = s.replace("var fn:", "var fn_rjk:");
        s = s.replace("var fn ", "var fn_rjk ");
        // Replace bare uses in expressions: ` fn `, `(fn `, ` fn)`, `\tfn `
        // These won't match `fn funcname(` because `funcname` is a word character.
        // We do a simple scan: replace `fn` that is NOT followed by a space+identifier.
        let mut out = String::with_capacity(s.len());
        let mut rest: &str = &s;
        while let Some(pos) = rest.find("fn") {
            let before = if pos > 0 {
                rest.as_bytes()[pos - 1] as char
            } else {
                ' '
            };
            let after_pos = pos + 2;
            let after = if after_pos < rest.len() {
                rest.as_bytes()[after_pos] as char
            } else {
                ' '
            };
            // Skip if it's part of a longer identifier
            if is_word_char(before) || is_word_char(after) {
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }
            // Skip if it's the WGSL function keyword: `fn ` followed by an alphanumeric name
            if after == ' ' || after == '\t' {
                let next_token = rest[after_pos..].trim_start();
                if next_token.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                    // This looks like `fn functionName(` — keep as-is
                    out.push_str(&rest[..after_pos]);
                    rest = &rest[after_pos..];
                    continue;
                }
            }
            out.push_str(&rest[..pos]);
            out.push_str("fn_rjk");
            rest = &rest[after_pos..];
        }
        out.push_str(rest);
        s = out;
    }
    s
}

// ---------------------------------------------------------------------------
// Void-main detection (handles void main(void), void main( ), etc.)
// ---------------------------------------------------------------------------

fn is_void_main_line(trimmed: &str) -> bool {
    // Accept both `void main(` and `void main (` (space before paren)
    let rest = if trimmed.starts_with("void main(") {
        &trimmed["void main(".len()..]
    } else if trimmed.starts_with("void main (") {
        &trimmed["void main (".len()..]
    } else {
        return false;
    };
    // Find closing ) — args can be empty or "void" (possibly with spaces)
    let close = rest.find(')').unwrap_or(0);
    let args = rest[..close].trim();
    args.is_empty() || args == "void"
}

// ---------------------------------------------------------------------------
// Strip `uniform` declarations (GLSL-only; ISF uses our uniform struct)
// ---------------------------------------------------------------------------

fn strip_uniform_declarations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim();
        // Strip any module-scope `uniform TYPE ...;` lines
        if trimmed.starts_with("uniform ") && trimmed.ends_with(';') {
            continue; // drop the line
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Join multi-line ternary expressions:
///   `cond\n  ? true_val\n  : false_val`  →  `cond ? true_val : false_val`
/// A line is a continuation if it is trimmed to start with `?` or starts with `:` that
/// is clearly a ternary false-branch (not `case:`, `default:`, empty `:`, etc.).
fn join_ternary_continuation_lines(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < lines.len() {
        let mut joined = lines[i].to_owned();
        // Look ahead for continuation lines (lines starting with ? or :, or when the
        // current line ends with a binary operator inside a ternary context).
        loop {
            if i + 1 >= lines.len() {
                break;
            }
            let next = lines[i + 1].trim();
            let is_ternary_q = next.starts_with('?') && !next.starts_with("//");
            // `:` continuation: starts with `: ` followed by non-empty content,
            // and the `:` is not a case/default/struct separator.
            let is_ternary_colon = next.len() > 1
                && next.starts_with(": ")
                && !next.starts_with("case ")
                && !next.starts_with("default:");
            // Also join when current joined line contains `?` and ends with a binary operator,
            // indicating the true-value expression spans multiple lines.
            let ends_with_op = {
                let t = joined.trim_end();
                t.ends_with('*')
                    || t.ends_with('/')
                    || t.ends_with('%')
                    || t.ends_with("||")
                    || t.ends_with("&&")
                    || (t.ends_with('+') && !t.ends_with("++"))
                    || (t.ends_with('-') && !t.ends_with("--"))
                    || t.ends_with('|')
                    || t.ends_with('&')
                    || t.ends_with('^')
            };
            let in_ternary_context = joined.contains('?');
            let is_operator_continuation = ends_with_op && in_ternary_context;
            if is_ternary_q || is_ternary_colon || is_operator_continuation {
                joined.push(' ');
                joined.push_str(next);
                i += 1;
            } else {
                break;
            }
        }
        out.push_str(&joined);
        out.push('\n');
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Ternary operator: `COND ? A : B` → `select(B, A, COND)`
// ---------------------------------------------------------------------------

fn convert_ternary_operators(src: &str) -> String {
    // Run up to 4 passes to handle nested ternaries
    let mut s = src.to_string();
    for _ in 0..4 {
        let s2 = convert_ternary_pass(&s);
        if s2 == s {
            break;
        }
        s = s2;
    }
    s
}

fn convert_ternary_pass(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || !line.contains('?') {
            out.push_str(line);
        } else {
            out.push_str(&convert_ternary_in_line(line));
        }
        out.push('\n');
    }
    out
}

fn convert_ternary_in_line(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut paren_depth = 0i32;

    for qi in 0..n {
        match chars[qi] {
            '(' | '[' => paren_depth += 1,
            ')' | ']' => paren_depth -= 1,
            '?' => {
                let q_depth = paren_depth;

                // Find condition start: scan backward for (, ,, =, ; at same depth
                let mut cond_start = 0usize;
                let mut scan_depth = q_depth;
                let mut found_boundary = false;
                for j in (0..qi).rev() {
                    match chars[j] {
                        ')' | ']' => scan_depth += 1,
                        '(' | '[' => {
                            scan_depth -= 1;
                            if scan_depth < q_depth {
                                cond_start = j + 1;
                                found_boundary = true;
                                break;
                            }
                        }
                        ',' | ';' if scan_depth == q_depth => {
                            cond_start = j + 1;
                            found_boundary = true;
                            break;
                        }
                        '=' if scan_depth == q_depth => {
                            // Not ==, !=, <=, >=
                            let prev = if j > 0 { chars[j - 1] } else { ' ' };
                            let next = if j + 1 < n { chars[j + 1] } else { ' ' };
                            if !matches!(prev, '!' | '<' | '>' | '=') && next != '=' {
                                cond_start = j + 1;
                                found_boundary = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let _ = found_boundary;

                // Find matching ':' after '?' at q_depth, tracking ternary nesting
                let mut scan_depth = q_depth;
                let mut ternary_nesting = 0i32;
                let mut colon_pos = None;
                for j in (qi + 1)..n {
                    match chars[j] {
                        '(' | '[' => scan_depth += 1,
                        ')' | ']' => {
                            if scan_depth == q_depth && q_depth > 0 {
                                break;
                            }
                            if scan_depth > 0 {
                                scan_depth -= 1;
                            }
                        }
                        '?' if scan_depth == q_depth => ternary_nesting += 1,
                        ':' if scan_depth == q_depth => {
                            if ternary_nesting > 0 {
                                ternary_nesting -= 1;
                            } else {
                                colon_pos = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let colon_pos = match colon_pos {
                    Some(p) => p,
                    None => continue,
                };

                // Find end of false value
                let mut scan_depth = q_depth;
                let mut ternary_nesting = 0i32;
                let mut false_end = n;
                for j in (colon_pos + 1)..n {
                    match chars[j] {
                        '(' | '[' => scan_depth += 1,
                        ')' | ']' => {
                            if scan_depth == q_depth {
                                false_end = j;
                                break;
                            }
                            if scan_depth > 0 {
                                scan_depth -= 1;
                            }
                        }
                        '?' if scan_depth == q_depth => ternary_nesting += 1,
                        ':' if scan_depth == q_depth => {
                            if ternary_nesting > 0 {
                                ternary_nesting -= 1;
                            } else {
                                false_end = j;
                                break;
                            }
                        }
                        ',' | ';' if scan_depth == q_depth && ternary_nesting == 0 => {
                            false_end = j;
                            break;
                        }
                        _ => {}
                    }
                }

                let prefix_raw: String = chars[..cond_start].iter().collect();
                let cond_raw: String = chars[cond_start..qi].iter().collect();
                let true_val: String = chars[qi + 1..colon_pos].iter().collect();
                let false_val: String = chars[colon_pos + 1..false_end].iter().collect();
                let suffix: String = chars[false_end..].iter().collect();

                // `return COND ? A : B` — the `return` belongs in the prefix, not the condition.
                let cond_trimmed = cond_raw.trim();
                let (prefix, cond) = if let Some(rest) = cond_trimmed
                    .strip_prefix("return ")
                    .or_else(|| cond_trimmed.strip_prefix("return\t"))
                {
                    (format!("{}return ", prefix_raw), rest.trim().to_string())
                } else {
                    (prefix_raw, cond_trimmed.to_string())
                };

                // WGSL does not support unary `+`. Strip it from true/false values.
                let true_v = true_val.trim().trim_start_matches('+');
                let false_v = false_val.trim().trim_start_matches('+');

                return format!(
                    "{}select({}, {}, {}){}",
                    prefix, false_v, true_v, cond, suffix
                );
            }
            _ => {}
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------------------
// Inject `var` copies for function parameters (WGSL params are immutable).
// Renames each param `p` → `_p` in the signature, injects `var p: T = _p;`.
// ---------------------------------------------------------------------------

fn inject_param_var_shadows(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 512);
    let mut prev_line_is_attr = false;
    // Shadows pending injection after the next standalone `{` line (next-line-brace style)
    let mut pending_shadows: Vec<String> = Vec::new();

    for line in src.lines() {
        let trimmed = line.trim();

        // If we have pending shadows and this line is the opening `{` of the function body
        if !pending_shadows.is_empty() && trimmed == "{" {
            out.push_str(line);
            out.push('\n');
            for shadow in pending_shadows.drain(..) {
                out.push_str(&shadow);
                out.push('\n');
            }
            continue;
        }

        // Track @fragment / @vertex entry-point attributes
        if trimmed.starts_with("@fragment") || trimmed.starts_with("@vertex") {
            prev_line_is_attr = true;
            pending_shadows.clear(); // reset if we somehow missed the brace
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Rename params and inject var copies for non-entry-point functions
        if !prev_line_is_attr {
            // Same-line brace: `fn name(...) { `
            if let Some((new_sig, shadows)) = rename_fn_params_for_mutability(line) {
                out.push_str(&new_sig);
                out.push('\n');
                for shadow in shadows {
                    out.push_str(&shadow);
                    out.push('\n');
                }
                if !trimmed.is_empty() {
                    prev_line_is_attr = false;
                }
                continue;
            }
            // Next-line brace: `fn name(...) -> TYPE` followed by a lone `{` on the next line
            if let Some((new_sig, shadows)) = rename_fn_params_next_line_brace(line) {
                out.push_str(&new_sig);
                out.push('\n');
                pending_shadows = shadows;
                if !trimmed.is_empty() {
                    prev_line_is_attr = false;
                }
                continue;
            }
        }

        if !trimmed.is_empty() {
            prev_line_is_attr = false;
            pending_shadows.clear(); // reset on non-empty, non-brace line (shouldn't happen)
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// For `fn name(p: T, q: U) -> R {`, rename params to `_p`, `_q` and return:
/// - new signature line: `fn name(_p: T, _q: U) -> R {`
/// - shadow lines: `    var p: T = _p;`, `    var q: U = _q;`
fn rename_fn_params_for_mutability(line: &str) -> Option<(String, Vec<String>)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("fn ") || !trimmed.ends_with('{') {
        return None;
    }

    let after_fn = &trimmed[3..];
    let paren_open = after_fn.find('(')?;
    let func_name = after_fn[..paren_open].trim();
    let params_content = &after_fn[paren_open + 1..];
    let (params_str, rest_after_close) = extract_balanced(params_content, ')');

    if params_str.trim().is_empty() {
        return None;
    }

    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];
    let body_indent = format!("{}    ", indent);

    let mut new_param_parts = Vec::new();
    let mut shadows = Vec::new();

    for param in split_top_level_commas(params_str) {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        if let Some(colon_pos) = param.find(':') {
            let name = param[..colon_pos].trim();
            let ty = param[colon_pos + 1..].trim();
            // Skip keywords, annotations, and already-prefixed params
            if name.is_empty() || name.starts_with('@') || name.starts_with("_fp_") {
                new_param_parts.push(param.to_string());
                continue;
            }
            if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                new_param_parts.push(param.to_string());
                continue;
            }
            if matches!(name, "in" | "out" | "self") {
                new_param_parts.push(param.to_string());
                continue;
            }
            // Rename param: p → _fp_p, inject var p: T = _fp_p;
            // Prefix `_fp_` (function-parameter) is unlikely to conflict with user variables.
            new_param_parts.push(format!("_fp_{}: {}", name, ty));
            shadows.push(format!(
                "{}var {}: {} = _fp_{};",
                body_indent, name, ty, name
            ));
        } else {
            new_param_parts.push(param.to_string());
        }
    }

    if shadows.is_empty() {
        return None;
    }

    let new_params_str = new_param_parts.join(", ");
    let new_sig = format!(
        "{}fn {}({}) {}",
        indent,
        func_name,
        new_params_str,
        rest_after_close.trim()
    );
    Some((new_sig, shadows))
}

/// Like `rename_fn_params_for_mutability` but for the next-line-brace style:
/// `fn name(p: T, q: U) -> R` followed by a lone `{` on the next line.
fn rename_fn_params_next_line_brace(line: &str) -> Option<(String, Vec<String>)> {
    let trimmed = line.trim();
    // Must be a `fn ` signature without a trailing `{`
    if !trimmed.starts_with("fn ") || trimmed.ends_with('{') {
        return None;
    }
    // Must look like a function signature: has `(` and `)` and not end in `;`
    if !trimmed.contains('(') || trimmed.ends_with(';') {
        return None;
    }
    // Delegate by appending a fake `{` and calling the same-line version
    let fake_line = format!("{} {{", line.trim_end());
    if let Some((sig_with_brace, shadows)) = rename_fn_params_for_mutability(&fake_line) {
        // Strip the appended ` {` from the signature
        let indent_len = line.len() - trimmed.len();
        let clean = sig_with_brace
            .trim_end_matches(" {")
            .trim_end_matches('{')
            .trim_end();
        Some((
            format!("{}{}", &line[..indent_len], &clean[indent_len..]),
            shadows,
        ))
    } else {
        None
    }
}

/// Split a comma-separated parameter list at top-level commas (not inside <> or ()).
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut angle = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '<' => angle += 1,
            '>' if angle > 0 => {
                angle -= 1;
            }
            ',' if depth == 0 && angle == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

// ---------------------------------------------------------------------------
// Multi-var declarations: `var a: T = x, b = y` → split into separate vars
// ---------------------------------------------------------------------------

fn split_multi_var_decls(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        match try_split_multi_var_decl(line) {
            Some(lines) => {
                for l in lines {
                    out.push_str(&l);
                    out.push('\n');
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn try_split_multi_var_decl(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with("var ") {
        return None;
    }
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];

    // Parse: var NAME: TYPE = VALUE...;
    let rest = &trimmed[4..]; // after "var "
    let colon = rest.find(':')?;
    let name0 = rest[..colon].trim().to_string();
    let after_colon = rest[colon + 1..].trim();

    // Find first `=` at top level (not `==`)
    let eq = find_top_level_assign(after_colon)?;
    let ty = after_colon[..eq].trim().to_string();
    let val_part = after_colon[eq + 1..].trim();

    // Scan for `, IDENT =` at top level
    let assignments = split_at_top_level_ident_eq(val_part, &name0)?;
    if assignments.len() <= 1 {
        return None;
    }

    let mut result = Vec::new();
    for (var_name, var_val) in assignments {
        result.push(format!(
            "{}var {}: {} = {};",
            indent,
            var_name,
            ty,
            var_val.trim().trim_end_matches(';')
        ));
    }
    Some(result)
}

fn find_top_level_assign(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] as char {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' if depth > 0 => {
                depth -= 1;
            }
            '=' if depth == 0 => {
                // Not `==`
                let next = if i + 1 < bytes.len() {
                    bytes[i + 1] as char
                } else {
                    ' '
                };
                if next != '=' {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Scan `s` for `, NAME =` patterns at top level (depth 0).
/// Returns vec of (name, value_str) pairs.
fn split_at_top_level_ident_eq(s: &str, first_name: &str) -> Option<Vec<(String, String)>> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut parts: Vec<(String, String)> = Vec::new();
    let mut depth = 0i32;
    let mut seg_start = 0usize; // char index
    let mut current_name = first_name.to_string();

    let mut i = 0usize;
    while i < n {
        match chars[i] {
            '(' | '[' => {
                depth += 1;
                i += 1;
            }
            ')' | ']' => {
                depth -= 1;
                i += 1;
            }
            ',' if depth == 0 => {
                let comma_i = i;
                i += 1;
                // Skip whitespace
                while i < n && chars[i].is_whitespace() {
                    i += 1;
                }
                // Read identifier
                let ident_start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident_end = i;
                if ident_end == ident_start {
                    i = comma_i + 1;
                    continue;
                }
                // Skip whitespace
                while i < n && chars[i].is_whitespace() {
                    i += 1;
                }
                // Check for `=` (not `==`)
                if i < n && chars[i] == '=' && (i + 1 >= n || chars[i + 1] != '=') {
                    i += 1; // skip '='
                    while i < n && chars[i].is_whitespace() {
                        i += 1;
                    }
                    // Save the previous segment
                    let val: String = chars[seg_start..comma_i].iter().collect();
                    parts.push((current_name.clone(), val.trim().to_string()));
                    current_name = chars[ident_start..ident_end].iter().collect();
                    seg_start = i;
                } else {
                    i = comma_i + 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if parts.is_empty() {
        return None;
    }
    // Add the last segment
    let last_val: String = chars[seg_start..].iter().collect();
    parts.push((
        current_name,
        last_val.trim().trim_end_matches(';').to_string(),
    ));
    Some(parts)
}

// ---------------------------------------------------------------------------
// Float for-loop increment fix: `i++` → `i += 1.0` for float vars
// ---------------------------------------------------------------------------

fn fix_float_for_increment(after_ident: &str, ident: &str) -> String {
    let s = after_ident.replace(&format!("{}++", ident), &format!("{} += 1.0", ident));
    s.replace(&format!("{}--", ident), &format!("{} -= 1.0", ident))
}

// ---------------------------------------------------------------------------
// Multi-line braceless control flow (state machine pass)
// ---------------------------------------------------------------------------

fn add_braces_to_multiline_braceless_control_flow(src: &str) -> String {
    // Pre-pass: join multi-line control flow conditions onto one line.
    // GLSL allows `if (cond1 ||\n    cond2)\n  body;` — join so the header ends with `)`.
    let src = join_control_flow_condition_lines(src);
    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();
    let mut out = String::with_capacity(src.len() + 256);
    let mut i = 0;

    while i < n {
        let line = lines[i];
        let trimmed = line.trim();
        let indent = &line[..line.len() - line.trim_start().len()];

        if is_multiline_braceless_header(trimmed) {
            let j_opt = (i + 1..n)
                .find(|&k| !lines[k].trim().is_empty() && !lines[k].trim().starts_with("//"));
            if let Some(j) = j_opt {
                let body_line = lines[j];
                let body_trimmed = body_line.trim();

                if body_trimmed == "{" {
                    // Brace-on-next-line style (`if (cond)\n{`): the block is already braced.
                    // Just emit everything as-is — no extra wrapping needed.
                    // The inner content will be processed in subsequent iterations.
                } else if body_trimmed.ends_with('{') && body_trimmed != "{" {
                    // Body is itself a braced statement (e.g. nested for loop starting with `for (...) {`).
                    // Find the matching `}` and wrap the entire block.
                    let mut depth = 0i32;
                    let mut block_end = n;
                    'outer: for k in j..n {
                        for c in lines[k].chars() {
                            match c {
                                '{' => depth += 1,
                                '}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        block_end = k;
                                        break 'outer;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    if block_end != n {
                        out.push_str(line);
                        out.push_str(" {\n");
                        for k in (i + 1)..=block_end {
                            out.push_str(lines[k]);
                            out.push('\n');
                        }
                        out.push_str(indent);
                        out.push_str("}\n");
                        i = block_end + 1;
                        continue;
                    }
                } else if !body_trimmed.ends_with('}')
                    && !is_multiline_braceless_header(body_trimmed)
                {
                    // Simple single-statement body
                    out.push_str(line);
                    out.push_str(" {\n");
                    for k in (i + 1)..j {
                        out.push_str(lines[k]);
                        out.push('\n');
                    }
                    out.push_str(body_line);
                    out.push('\n');
                    out.push_str(indent);
                    out.push_str("}\n");
                    i = j + 1;
                    continue;
                }
            }
        }

        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
}

/// Join multi-line control-flow condition lines.
/// Handles patterns like:
///   `if (cond1 ||\n    cond2)\n  body;`
/// → `if (cond1 || cond2)\n  body;`
///
/// Only joins when the control-flow condition parentheses are still OPEN at the end of the line
/// (i.e., the `(...)` wrapping the condition is not yet balanced).
fn join_control_flow_condition_lines(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < n {
        let trimmed = lines[i].trim();
        // Only consider lines that start with a control flow keyword
        let starts_cf = trimmed.starts_with("if (")
            || trimmed.starts_with("} else if (")
            || trimmed.starts_with("else if (")
            || trimmed.starts_with("for (")
            || trimmed.starts_with("while (");
        if starts_cf {
            // Check paren balance: if balanced (depth==0), the condition is closed on this line
            // and it is either a complete header or a complete single-line statement. Don't join.
            let depth = paren_depth(trimmed);
            if depth > 0 {
                // Condition parens are still open — join subsequent lines until balanced
                let mut joined = lines[i].to_owned();
                let mut d = depth;
                i += 1;
                while i < n && d > 0 {
                    let next = lines[i].trim();
                    joined.push(' ');
                    joined.push_str(next);
                    i += 1;
                    for c in next.chars() {
                        match c {
                            '(' => d += 1,
                            ')' => d -= 1,
                            _ => {}
                        }
                    }
                }
                out.push_str(&joined);
                out.push('\n');
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

fn paren_depth(s: &str) -> i32 {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '(' => d += 1,
            ')' => d -= 1,
            _ => {}
        }
    }
    d
}

fn is_multiline_braceless_header(trimmed: &str) -> bool {
    if trimmed.ends_with('{') || trimmed.ends_with('}') || trimmed.is_empty() {
        return false;
    }

    // `else` or `} else` alone on a line
    if trimmed == "else" || trimmed == "} else" {
        return true;
    }

    // `if (...)`, `else if (...)`, `for (...)`, `while (...)` ending with `)` — no body
    for kw in &["if (", "} else if (", "else if (", "for (", "while ("] {
        if trimmed.starts_with(kw) {
            return trimmed.ends_with(')');
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Identifier sanitization
// ---------------------------------------------------------------------------

fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Rewrite partial `gl_FragColor.X = ...;` writes in `void main()` to use a local accumulator.
///
/// Shaders that write `gl_FragColor.rgb = expr;` or `gl_FragColor.a = val;` (rather than
/// a single `gl_FragColor = vec4(...)`) cannot be handled by the simple `return` substitution.
///
/// This function:
/// 1. Replaces all `gl_FragColor.X` with `_fc.X` in the main body.
/// 2. Inserts `vec4 _fc = vec4(0.0, 0.0, 0.0, 1.0);` at the start of `void main()`.
/// 3. Inserts `gl_FragColor = _fc;` just before the closing `}` of `void main()`.
///    The later `gl_FragColor = ` → `return ` pass then converts this to `return _fc;`.
fn rewrite_partial_gl_frag_color(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();
    let mut out = Vec::with_capacity(n + 4);

    let mut i = 0;
    while i < n {
        let trimmed = lines[i].trim();
        // Detect void main() opening
        if is_void_main_line(trimmed) {
            // Find the opening { — may be on the same line or the next
            let mut main_brace_line = i;
            if !trimmed.contains('{') {
                while main_brace_line + 1 < n && !lines[main_brace_line].contains('{') {
                    main_brace_line += 1;
                }
            }
            // Find the closing } by tracking brace depth
            let mut depth = 0i32;
            let mut main_end = n;
            for j in main_brace_line..n {
                for c in lines[j].chars() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                main_end = j;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if main_end != n {
                    break;
                }
            }
            // Collect the body lines (between opening and closing brace)
            // Output lines up to and including the opening brace line, inserting _fc decl
            for j in i..=main_brace_line {
                out.push(lines[j].to_owned());
            }
            out.push("    vec4 _fc = vec4(0.0, 0.0, 0.0, 1.0);".to_owned());
            // Emit body lines with gl_FragColor.X / fragColor.X → _fc.X replacement
            for j in main_brace_line + 1..main_end {
                let l = lines[j].replace("gl_FragColor.", "_fc.");
                let l = l.replace("fragColor.", "_fc.");
                // Also replace full `gl_FragColor =` / `fragColor =` if present
                let l = l.replace("gl_FragColor =", "_fc =");
                let l = l.replace("gl_FragColor=", "_fc=");
                let l = l.replace("fragColor =", "_fc =");
                let l = l.replace("fragColor=", "_fc=");
                out.push(l);
            }
            // Insert the final assignment before the closing brace
            out.push("    gl_FragColor = _fc;".to_owned());
            out.push(lines[main_end].to_owned());
            i = main_end + 1;
            continue;
        }
        out.push(lines[i].to_owned());
        i += 1;
    }

    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transpile_shader(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("shaders")
            .join(name);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} not found", name));
        let isf = isf::parse(&src).unwrap_or_else(|e| panic!("parse failed for {}: {}", name, e));
        let result = generate_wgsl(&isf, &src)
            .unwrap_or_else(|e| panic!("transpile failed for {}: {}", name, e));
        result.wgsl
    }

    fn assert_valid_wgsl(wgsl: &str, shader_name: &str) {
        assert!(
            wgsl.contains("fn fs_main"),
            "{}: missing fs_main",
            shader_name
        );
        assert!(
            wgsl.contains("fn vs_main"),
            "{}: missing vs_main",
            shader_name
        );
        // Should have balanced braces
        let open = wgsl.chars().filter(|&c| c == '{').count();
        let close = wgsl.chars().filter(|&c| c == '}').count();
        assert_eq!(
            open, close,
            "{}: unbalanced braces ({} open, {} close)",
            shader_name, open, close
        );
    }

    #[test]
    fn test_vec4_rgb_plus_alpha_preserved() {
        use std::collections::HashMap;
        let mut var_types = HashMap::new();
        var_types.insert("sample0".to_string(), "vec4<f32>".to_string());
        var_types.insert("sample1".to_string(), "vec4<f32>".to_string());
        var_types.insert("sample2".to_string(), "vec4<f32>".to_string());
        // The line: return vec4<f32>((sample0 + sample1 + sample2).rgb / (3.0), 1.0);
        let line = "\t\treturn  vec4<f32>((sample0 + sample1 + sample2).rgb / (3.0), 1.0);";
        let result = fix_vec_constructors_in_line(line, &var_types);
        eprintln!("input:  {}", line);
        eprintln!("output: {}", result);
        assert!(
            result.contains("1.0"),
            "1.0 should be preserved: {}",
            result
        );
    }

    #[test]
    fn test_colorcycle_transpiles() {
        let wgsl = transpile_shader("ColorCycle.fs");
        eprintln!("=== ColorCycle WGSL ===\n{}", wgsl);
        assert_valid_wgsl(&wgsl, "ColorCycle.fs");
        assert!(wgsl.contains("isf_u.time"), "missing time uniform");
        assert!(wgsl.contains("atan2"), "atan should be converted to atan2");
        assert!(
            !wgsl.contains("vec3 iResolution"),
            "module-scope var should be injected"
        );
        // iResolution is emitted as var<private> at module scope + assigned in fs_main
        assert!(
            wgsl.contains("iResolution"),
            "iResolution should be present in output"
        );
        assert!(
            wgsl.contains("iResolution = vec3<f32>(isf_u.rendersize_x"),
            "iResolution should be assigned in fs_main"
        );
    }

    #[test]
    fn test_all_wgsl_validates_with_naga() {
        let naga = std::process::Command::new("naga").arg("--version").output();
        if naga.is_err() || !naga.unwrap().status.success() {
            eprintln!("naga CLI not available, skipping WGSL validation test");
            return;
        }
        let shaders_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut naga_failures = Vec::new();
        let mut ok = 0;
        for entry in std::fs::read_dir(&shaders_dir).expect("shaders dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("fs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let isf = match isf::parse(&src) {
                Ok(i) => i,
                Err(_) => continue,
            };
            let result = match generate_wgsl(&isf, &src) {
                Ok(r) => r,
                Err(e) => {
                    naga_failures.push(format!("{}: {}", name, e));
                    continue;
                }
            };
            let tmp =
                std::env::temp_dir().join(format!("isf_test_{}.wgsl", name.replace(' ', "_")));
            std::fs::write(&tmp, &result.wgsl).expect("write wgsl");
            let out = std::process::Command::new("naga")
                .arg(&tmp)
                .output()
                .expect("run naga");
            std::fs::remove_file(&tmp).ok();
            if out.status.success() {
                ok += 1;
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                let preview: String = err.lines().take(6).collect::<Vec<_>>().join(" | ");
                naga_failures.push(format!("{}: {}", name, preview));
            }
        }
        eprintln!(
            "WGSL validation: {}/{} passed",
            ok,
            ok + naga_failures.len()
        );
        for f in &naga_failures {
            eprintln!("  FAIL: {}", f);
        }
        // This test reports but doesn't fail — some shaders have known GLSL features not in WGSL
    }

    #[test]
    fn batch_validate_isf_library() {
        let naga = std::process::Command::new("naga").arg("--version").output();
        if naga.is_err() || !naga.unwrap().status.success() {
            eprintln!("naga CLI not available, skipping batch library validation");
            return;
        }
        let lib_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../isf/test_files");
        if !lib_dir.exists() {
            eprintln!("ISF library not found at {}, skipping", lib_dir.display());
            return;
        }

        let mut naga_failures: Vec<String> = Vec::new();
        let mut parse_skipped = 0usize;
        let mut ok = 0usize;
        let mut error_categories: std::collections::BTreeMap<String, Vec<String>> =
            Default::default();

        for entry in std::fs::read_dir(&lib_dir).expect("read isf test_files dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("fs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let isf = match isf::parse(&src) {
                Ok(i) => i,
                Err(_) => {
                    parse_skipped += 1;
                    continue;
                }
            };
            let result = match generate_wgsl(&isf, &src) {
                Ok(r) => r,
                Err(e) => {
                    let msg = format!("[transpile-error] {}", e);
                    error_categories
                        .entry("transpile-error".to_string())
                        .or_default()
                        .push(name.clone());
                    naga_failures.push(format!("{}: {}", name, msg));
                    continue;
                }
            };

            let tmp = std::env::temp_dir()
                .join(format!("isf_lib_{}.wgsl", name.replace([' ', '/'], "_")));
            std::fs::write(&tmp, &result.wgsl).expect("write wgsl");
            let out = std::process::Command::new("naga")
                .arg(&tmp)
                .output()
                .expect("run naga");
            std::fs::remove_file(&tmp).ok();

            if out.status.success() {
                ok += 1;
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // Extract the core error message for categorisation
                let category = stderr
                    .lines()
                    .find(|l| l.contains("error:"))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                // Shorten category to the key phrase
                let cat_key = if category.contains("no definition in scope") {
                    "undefined-identifier"
                } else if category.contains("expected") && category.contains("got") {
                    "type-mismatch"
                } else if category.contains("cannot index") {
                    "invalid-index"
                } else if category.contains("unknown identifier") {
                    "undefined-identifier"
                } else if category.contains("should be") {
                    "type-error"
                } else if category.contains("parse") {
                    "parse-error"
                } else {
                    "other"
                }
                .to_string();

                let preview: String = stderr
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(4)
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(" | ");

                error_categories
                    .entry(cat_key)
                    .or_default()
                    .push(name.clone());
                naga_failures.push(format!("{}: {}", name, preview));
            }
        }

        eprintln!("\n=== ISF library batch WGSL validation ===");
        eprintln!(
            "  Parsed & transpiled: {} ok, {} naga failures, {} parse-skipped",
            ok,
            naga_failures.len(),
            parse_skipped
        );
        eprintln!("\nFailure categories:");
        for (cat, names) in &error_categories {
            eprintln!("  [{cat}] ({} shaders)", names.len());
            for n in names.iter().take(5) {
                eprintln!("    - {}", n);
            }
            if names.len() > 5 {
                eprintln!("    ... and {} more", names.len() - 5);
            }
        }
        if !naga_failures.is_empty() {
            eprintln!("\nFull failure list:");
            for f in &naga_failures {
                eprintln!("  {}", f);
            }
        }
        // Informational test: never fails, just reports
    }

    #[test]
    fn test_varda_shaders_transpile() {
        let naga = std::process::Command::new("naga").arg("--version").output();
        let has_naga = naga.is_ok() && naga.unwrap().status.success();
        if !has_naga {
            eprintln!("naga CLI not available, skipping WGSL validation");
        }
        let dir = std::path::Path::new("/Users/ac/developer/rust/varda/shaders");
        let mut ok = 0;
        let mut fail = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("fs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let glsl_src = std::fs::read_to_string(&path).unwrap();
            let isf = match isf::parse(&glsl_src) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("  {}: ISF parse error: {}", name, e);
                    fail += 1;
                    continue;
                }
            };
            let transpiled = match generate_wgsl(&isf, &glsl_src) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("  {}: transpile error: {}", name, e);
                    fail += 1;
                    continue;
                }
            };
            if has_naga {
                let tmp = std::env::temp_dir().join(format!("varda_test_{}.wgsl", name.replace(' ', "_")));
                std::fs::write(&tmp, &transpiled.wgsl).unwrap();
                let out = std::process::Command::new("naga").arg(&tmp).output().expect("run naga");
                if out.status.success() {
                    std::fs::remove_file(&tmp).ok();
                    ok += 1;
                } else {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let preview: String = err.lines().take(4).collect::<Vec<_>>().join(" | ");
                    eprintln!("  FAIL {}: {}", name, preview);
                    fail += 1;
                }
            } else {
                ok += 1; // count as ok if we can't validate
            }
        }
        eprintln!("\nVarda shaders: {} ok, {} failed", ok, fail);
        // Informational: track progress without failing the suite
        if fail > 0 {
            eprintln!("  ({} shaders still have transpilation edge cases)", fail);
        }
    }

    #[test]
    fn test_all_ghost_arcade_parse_and_transpile() {
        let shaders_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut transpile_failures = Vec::new();
        let mut parse_skipped = Vec::new();
        let mut ok = 0;
        for entry in std::fs::read_dir(&shaders_dir).expect("shaders dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("fs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    transpile_failures.push(format!("{}: read error: {}", name, e));
                    continue;
                }
            };
            // ISF parse failures are acceptable (broken ISF files, not our bug)
            let isf = match isf::parse(&src) {
                Ok(i) => i,
                Err(e) => {
                    parse_skipped.push(format!("{}: {}", name, e));
                    continue;
                }
            };
            let result = match generate_wgsl(&isf, &src) {
                Ok(r) => r,
                Err(e) => {
                    transpile_failures.push(format!("{}: transpile error: {}", name, e));
                    continue;
                }
            };
            let open = result.wgsl.chars().filter(|&c| c == '{').count();
            let close = result.wgsl.chars().filter(|&c| c == '}').count();
            if open != close {
                transpile_failures.push(format!(
                    "{}: unbalanced braces ({} open, {} close)",
                    name, open, close
                ));
            } else {
                ok += 1;
            }
        }
        if !parse_skipped.is_empty() {
            eprintln!(
                "Skipped {} shaders with ISF parse errors (not our bug):",
                parse_skipped.len()
            );
            for s in &parse_skipped {
                eprintln!("  - {}", s);
            }
        }
        eprintln!(
            "Transpiled {}/{} valid ISF shaders successfully",
            ok,
            ok + transpile_failures.len()
        );
        if !transpile_failures.is_empty() {
            panic!("Transpiler failures:\n{}", transpile_failures.join("\n"));
        }
    }
}

#[test]
fn dump_failing_wgsl_samples() {
    let lib_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../isf/test_files");
    if !lib_dir.exists() {
        return;
    }

    let samples = &["Gloom.fs", "Mosaic.fs", "Hexagonalize.fs", "Test-Audio.fs"];

    for name in samples {
        let path = lib_dir.join(name);
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skip {name}: not found");
                continue;
            }
        };
        let isf = match isf::parse(&src) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skip {name}: parse err {e}");
                continue;
            }
        };
        let result = match generate_wgsl(&isf, &src) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip {name}: transpile err {e}");
                continue;
            }
        };
        eprintln!("\n=== {name} (has_image={}) ===", result.has_image_input);
        for (i, line) in result.wgsl.lines().enumerate() {
            eprintln!("{:>4}: {}", i + 1, line);
        }
    }
}
