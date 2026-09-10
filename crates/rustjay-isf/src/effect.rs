//! `IsfEffect` — loads an ISF GLSL shader at runtime, parses its inputs,
//! compiles to WGSL (Phase 1 compile core), and renders via a custom pipeline.
//!
//! GPU ABI (single bind group, set 0 — see `crate::compile`):
//! binding 0 = IsfData uniform block (64 B), binding 1 = IsfInputs (when non-empty),
//! binding 2 = img_sampler (when textures exist), bindings 3+ = texture2D per
//! image/audio input, then PASSES targets and IMPORTED names.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use isf::Isf;
use rustjay_core::{
    BEAT_DIVISIONS, EffectPlugin, EngineState, ParamCategory, ParameterDescriptor, Vertex,
    lfo::BEAT_DIVISION_NAMES,
};
use wgpu::util::DeviceExt;

use crate::{
    compile::{self, FieldTy, IsfManifest, MAX_ISF_UNIFORMS},
    params::isf_inputs_to_parameters,
};

// ---------------------------------------------------------------------------
// State (serialisable parameter values keyed by ISF input name)
// ---------------------------------------------------------------------------

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct IsfState {
    pub values: HashMap<String, f32>,
}

// ---------------------------------------------------------------------------
// Uniforms: vestigial Pod type kept for the EffectPlugin trait bound.
// The real uniform data is std140-packed per frame in render().
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IsfUniforms([f32; MAX_ISF_UNIFORMS]);

// ---------------------------------------------------------------------------
// Pacing: parameters the shader does not declare
// ---------------------------------------------------------------------------

/// Multiplier on the instance's clock. 1.0 is real time.
pub const TIME_SPEED: &str = "time_speed";
/// Run the clock off the tempo rather than the wall clock.
pub const TIME_SYNC: &str = "time_sync";
/// Beats per shader-second while synced — indexes [`BEAT_DIVISIONS`].
pub const TIME_DIV: &str = "time_div";
/// Restart the clock. A trigger, not a setting: any *change* is one restart,
/// so the UI (or a mapped MIDI button) just toggles it.
pub const TIME_RESET: &str = "time_reset";

/// The `.fs` to load from what the user picked.
///
/// ISF downloads unzip to a bundle folder named `Whatever.fs` holding
/// `Whatever.fs.fs` (plus a `.vs` and sample images), so the thing that looks
/// like the shader is a directory. Point at either and get the shader.
fn resolve_shader_path(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "fs"))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Whether the shader's output moves on its own. Only a shader that reads the
/// clock — the TIME built-ins, a `PHASE_INPUTS` accumulator, or a MadMapper
/// time generator — has any pacing to control; a filter that just tints its
/// input does not, and a Speed slider on that is noise in the inspector.
///
/// `DATE` and `FRAMEINDEX` deliberately do not count: neither is scaled by the
/// clock, so pacing controls would do nothing for a shader driven by them.
fn uses_clock(glsl_src: &str) -> bool {
    ["TIMEDELTA", "PHASE_TIME", "PHASE_INPUTS", "time_base", "animator"]
        .iter()
        .any(|k| glsl_src.contains(k))
        || mentions_word(glsl_src, "TIME")
}

/// Whether `word` appears in `src` other than as part of a longer identifier.
/// Without this, every shader with a `lifetime` or a `RUNTIME` in it would be
/// taken for a clock-driven one.
fn mentions_word(src: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    src.match_indices(word).any(|(i, _)| {
        !src[..i].chars().next_back().is_some_and(ident)
            && !src[i + word.len()..].chars().next().is_some_and(ident)
    })
}

/// How fast the instance's clock runs this frame, in shader-seconds per real
/// second. Synced, one shader-second spans `BEAT_DIVISIONS[division]` beats, so
/// a shader looping on `mod(TIME, 1.0)` comes round once per division.
fn clock_rate(speed: f32, sync: bool, division: usize, bpm: f32) -> f32 {
    // No tempo yet (no audio, Link not joined) means no grid to lock to, so the
    // clock free-runs rather than crawling.
    if !sync || bpm <= 0.0 {
        return speed;
    }
    let beats = BEAT_DIVISIONS[division.min(BEAT_DIVISIONS.len() - 1)];
    speed * bpm / 60.0 / beats
}

/// Pacing controls added to every ISF instance. Prefixed like any other
/// parameter, so each layer keeps its own — and modulatable, mappable and
/// saved with the scene for free.
fn time_parameters() -> Vec<ParameterDescriptor> {
    let category = ParamCategory::Custom("ISF".to_string());
    vec![
        ParameterDescriptor::float(TIME_SPEED, "Speed", category.clone(), 0.0, 4.0, 1.0, 0.01),
        ParameterDescriptor::bool(TIME_SYNC, "Sync", category.clone(), false),
        ParameterDescriptor::bool(TIME_RESET, "Restart", category.clone(), false),
        ParameterDescriptor::enum_param(
            TIME_DIV,
            "Division",
            category,
            BEAT_DIVISION_NAMES.iter().map(|s| s.to_string()).collect(),
            // One whole note — a bar in 4/4, the length most loops want.
            4,
        ),
    ]
}

// ---------------------------------------------------------------------------
// IsfEffect
// ---------------------------------------------------------------------------

pub struct IsfEffect {
    pub isf: Isf,
    pub glsl_src: String,
    pub shader_name: String,

    /// Path to the source `.fs` file — used for hot reload.
    shader_path: PathBuf,
    /// Last-seen mtime of the file — used to detect changes.
    last_mtime: Option<SystemTime>,
    /// Shared with IsfTab: current shader display name (updated on every swap).
    pub shader_name_shared: Arc<Mutex<String>>,
    /// Shared with IsfTab: set to Some(path) to trigger loading a new shader.
    pub pending_path: Arc<Mutex<Option<PathBuf>>>,
    /// Set to true after a successful init() so the engine re-reads parameters().
    params_dirty: bool,

    /// `PHASE_INPUTS` from the shader header: which parameter drives which
    /// accumulator, and by how much.
    phase_inputs: Vec<PhaseInput>,
    /// `PHASE_TIME_0..3`. Integrated per frame rather than derived from TIME,
    /// which is the whole point: `TIME * speed` jumps when the speed changes,
    /// an accumulator carries on smoothly from where it was.
    phase: [f32; 4],
    /// Current value of each MadMapper generator, in manifest order. Advanced
    /// once per frame by [`IsfEffect::advance_generators`].
    generators: Vec<f32>,

    /// Whether this shader reads the clock at all — see [`uses_clock`]. False
    /// means no pacing parameters are declared.
    uses_clock: bool,
    /// Last seen [`TIME_RESET`] value; a change restarts the clock.
    last_reset: f32,
    /// The instance's own clock, in shader-seconds — the TIME built-in.
    /// Integrated rather than read off the wall clock so [`TIME_SPEED`] and
    /// [`TIME_SYNC`] can stretch it without TIME jumping when they change.
    time: f32,
    /// Previous frame's timestamp — for TIMEDELTA.
    last_frame: Option<Instant>,
    /// Pin the per-frame time step instead of measuring the wall clock.
    ///
    /// The clock is otherwise driven by `Instant::now()`, so two renders of the
    /// same shader never match and nothing can be compared frame to frame. A
    /// headless harness sets this to make a render reproducible.
    pub fixed_delta: Option<f32>,
    /// FRAMEINDEX built-in counter.
    frame_index: u64,

    /// Error message from transpilation / compilation (shown in GUI).
    pub transpile_error: Option<String>,

    /// Render to a target of this size rather than the engine's output, and
    /// report it as `RENDERSIZE`. Set by a host that owns its own target — a
    /// laser deck sizes it `POINT_COUNT` by [`crate::compile::LASER_ROWS`],
    /// which is how the shader learns its point budget.
    pub offscreen_size: Option<[u32; 2]>,
    /// Colour format of that target, when it is not the engine's working one.
    /// A laser material writes positions in -1..1, so an 8-bit unorm target
    /// would clamp them and quantise the beam to 256 steps.
    pub offscreen_format: Option<wgpu::TextureFormat>,

    // GPU resources (created in init())
    pipeline: Option<wgpu::RenderPipeline>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    vertex_buffer: Option<wgpu::Buffer>,
    /// IsfData block (binding 0), always 64 bytes — one per pass, because
    /// PASSINDEX differs per pass and a queue write only lands once per submit.
    data_buffers: Vec<wgpu::Buffer>,
    /// IsfInputs block (binding 1), present when inputs_block_size > 0.
    inputs_buffer: Option<wgpu::Buffer>,
    /// 1×1 black placeholder for unbound texture inputs.
    placeholder_view: Option<wgpu::TextureView>,
    /// One offscreen texture per `PASSES` target, in header order.
    pass_targets: Vec<PassTarget>,
    /// Copies the last pass's target to the engine's view, for a shader whose
    /// last pass renders into a buffer (`Test-PersistentBuffer` and every other
    /// one-pass feedback shader). Only built when that is the case.
    blit: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// Which half of each persistent target is this frame's write side.
    ping: usize,
    /// Our own filtering sampler (the GLSL constructs sampler2D(t, img_sampler)).
    sampler: Option<wgpu::Sampler>,

    manifest: Option<IsfManifest>,
    /// Precomputed (offset, type, lookup-keys) per IsfInputs field — avoids
    /// per-frame `format!` when reading params/state.
    pack_fields: Vec<PackField>,
    /// Texture input that receives the upstream frame: "inputImage" when present,
    /// else the first image/audio input. None for pure generators.
    primary_texture: Option<String>,
    /// The next image input after `primary_texture`, in declaration order.
    ///
    /// Bound to `RenderHookCtx::input_b` when the host supplies one, so a
    /// transition gets two distinct images instead of the same one twice.
    /// Order, not name: corpora disagree on the naming
    /// (`startImage`/`endImage`, `inputImage2`, `from`/`to`), but they all
    /// declare the incoming image first and the other second.
    secondary_texture: Option<String>,
}

/// One `PASSES` target: an offscreen texture the shader renders into and can
/// sample by name.
///
/// A persistent target is double-buffered: the pass writes `views[ping]` while
/// every sample of that target reads `views[ping ^ 1]`, last frame's content.
/// That is the ISF feedback idiom, and it is also what makes a delay line work
/// (`bufC <- bufB`, `bufB <- bufA`, `bufA <- input`) regardless of pass order.
/// A non-persistent target is a within-frame temporary: one texture, read back
/// as whatever an earlier pass wrote this frame.
struct PassTarget {
    name: String,
    persistent: bool,
    size: [u32; 2],
    /// One view for a temporary, two for a persistent (write/read halves).
    views: Vec<wgpu::TextureView>,
}

impl PassTarget {
    fn write_view(&self, ping: usize) -> &wgpu::TextureView {
        &self.views[ping % self.views.len()]
    }
    fn read_view(&self, ping: usize) -> &wgpu::TextureView {
        &self.views[(ping ^ 1) % self.views.len()]
    }
}

/// A std140 field with its precomputed state/param lookup keys.
struct PackField {
    offset: usize,
    ty: FieldTy,
    /// Component keys: scalar fields use only `k[0]`; vec2 uses `k[0..2]`
    /// (`name_x`, `name_y`); vec4 uses `k[0..4]` (`name_r.._a`).
    k: [String; 4],
    /// Set for `PHASE_TIME_0..3`, which come from the instance's accumulators
    /// rather than from a parameter of that name — there is none.
    phase: Option<usize>,
    /// Set for a MadMapper generator field, which the host drives for the same
    /// reason: it is not a parameter the user sets.
    generator: Option<usize>,
}

/// One `PHASE_INPUTS` entry: a parameter that drives an accumulator.
#[derive(Clone, Debug)]
struct PhaseInput {
    param: String,
    index: usize,
    scale: f32,
}

/// Read `PHASE_INPUTS` out of the ISF header comment.
///
/// The `isf` crate drops keys it does not know, and this one is a rustjay
/// extension, so the header JSON is read again here.
fn parse_phase_inputs(glsl_src: &str) -> Vec<PhaseInput> {
    // Scan comment blocks rather than assuming the first one: a shader may
    // carry a licence header above its ISF blob, and taking that one would
    // silently drop the phase inputs.
    let mut rest = glsl_src;
    let value = loop {
        let Some(start) = rest.find("/*") else {
            return Vec::new();
        };
        let Some(end) = rest[start..].find("*/") else {
            return Vec::new();
        };
        let body = rest[start + 2..start + end].trim();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
            && v.is_object()
        {
            break v;
        }
        rest = &rest[start + end + 2..];
    };
    value
        .get("PHASE_INPUTS")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let index = e.get("INDEX").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    // Four accumulators exist; anything else would be a silent
                    // out-of-bounds at render time.
                    if index >= 4 {
                        return None;
                    }
                    Some(PhaseInput {
                        param: e.get("PARAM")?.as_str()?.to_string(),
                        index,
                        scale: e.get("SCALE").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl IsfEffect {
    /// Allocate (or resize) one texture per `PASSES` target.
    ///
    /// ponytail: every target gets the render size and the working format —
    /// a pass's WIDTH/HEIGHT expressions and FLOAT flag are ignored. Add them
    /// when a shader needs a half-res or HDR intermediate (FLOAT also needs a
    /// second pipeline, since the format is baked into it).
    fn ensure_pass_targets(
        &mut self,
        device: &wgpu::Device,
        size: [u32; 2],
        lookup: &dyn Fn(&str) -> Option<f32>,
    ) {
        let want: Vec<(String, bool, [u32; 2])> = self
            .isf
            .passes
            .iter()
            .filter_map(|p| {
                let dim = |expr: &Option<String>, i: usize| {
                    expr.as_deref()
                        .and_then(|e| eval_size_expr(e, size, lookup))
                        .map_or(size[i], |v| (v as u32).clamp(1, 16384))
                };
                Some((
                    p.target.clone()?,
                    p.persistent,
                    [dim(&p.width, 0), dim(&p.height, 1)],
                ))
            })
            .collect();
        let fresh = self.pass_targets.len() == want.len()
            && self
                .pass_targets
                .iter()
                .zip(&want)
                .all(|(t, (_, _, s))| t.size == *s);
        if fresh {
            return;
        }
        let format = self
            .offscreen_format
            .unwrap_or_else(rustjay_core::working_format);
        self.pass_targets = want
            .into_iter()
            .enumerate()
            .map(|(i, (name, persistent, size))| {
                let views = (0..if persistent { 2 } else { 1 })
                    .map(|half| {
                        device
                            .create_texture(&wgpu::TextureDescriptor {
                                label: Some(&format!("ISF Pass {i} {name} {half}")[..]),
                                size: wgpu::Extent3d {
                                    width: size[0],
                                    height: size[1],
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format,
                                usage: wgpu::TextureUsages::TEXTURE_BINDING
                                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                                view_formats: &[],
                            })
                            .create_view(&wgpu::TextureViewDescriptor::default())
                    })
                    .collect();
                PassTarget {
                    name,
                    persistent,
                    size,
                    views,
                }
            })
            .collect();
    }

    /// Whether `name` is one of the shader's `image` inputs.
    fn is_image_input(&self, name: &str) -> bool {
        self.isf
            .inputs
            .iter()
            .any(|i| i.name == name && matches!(i.ty, isf::InputType::Image))
    }

    /// What the shader compiled to, once [`EffectPlugin::init`] has run.
    ///
    /// `None` before init, or when compilation failed — see `transpile_error`.
    pub fn manifest(&self) -> Option<&IsfManifest> {
        self.manifest.as_ref()
    }

    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let path = &resolve_shader_path(path);
        let glsl_src = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path.display(), e))?;
        let isf = crate::header::parse(&glsl_src)
            .map_err(|e| anyhow::anyhow!("ISF parse error in {}: {}", path.display(), e))?;
        let shader_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ISF Shader")
            .to_string();

        let phase_inputs = parse_phase_inputs(&glsl_src);
        let uses_clock = uses_clock(&glsl_src);
        Ok(Self {
            isf,
            glsl_src,
            shader_name_shared: Arc::new(Mutex::new(shader_name.clone())),
            shader_name,
            shader_path: path.to_path_buf(),
            last_mtime: std::fs::metadata(path).ok().and_then(|m| m.modified().ok()),
            pending_path: Arc::new(Mutex::new(None)),
            params_dirty: false,
            phase_inputs,
            phase: [0.0; 4],
            uses_clock,
            last_reset: 0.0,
            time: 0.0,
            last_frame: None,
            fixed_delta: None,
            frame_index: 0,
            transpile_error: None,
            offscreen_size: None,
            offscreen_format: None,
            pipeline: None,
            bind_group_layout: None,
            vertex_buffer: None,
            data_buffers: Vec::new(),
            inputs_buffer: None,
            placeholder_view: None,
            pass_targets: Vec::new(),
            blit: None,
            ping: 0,
            sampler: None,
            manifest: None,
            generators: Vec::new(),
            pack_fields: Vec::new(),
            primary_texture: None,
            secondary_texture: None,
        })
    }

    /// std140-pack the IsfInputs block from engine params (float/bool/long) and
    /// state values (color/point2D component keys, aux fields default to 0).
    fn pack_inputs(&self, state: &IsfState, engine: &EngineState) -> Vec<u8> {
        let Some(manifest) = &self.manifest else {
            return Vec::new();
        };
        let mut buf = vec![0u8; manifest.inputs_block_size];
        for f in &self.pack_fields {
            if let Some(n) = f.phase {
                put_f32(&mut buf, f.offset, self.phase[n]);
                continue;
            }
            if let Some(n) = f.generator {
                put_f32(&mut buf, f.offset, self.generators[n]);
                continue;
            }
            let get = |i: usize| {
                engine
                    .get_param(&f.k[i])
                    .or_else(|| state.values.get(&f.k[i]).copied())
                    .unwrap_or(0.0)
            };
            match f.ty {
                FieldTy::F32 => put_f32(&mut buf, f.offset, get(0)),
                FieldTy::I32 => put_i32(&mut buf, f.offset, get(0) as i32),
                FieldTy::Bool => put_u32(&mut buf, f.offset, (get(0) != 0.0) as u32),
                FieldTy::Vec2 => {
                    put_f32(&mut buf, f.offset, get(0));
                    put_f32(&mut buf, f.offset + 4, get(1));
                }
                FieldTy::Vec3 => {
                    put_f32(&mut buf, f.offset, get(0));
                    put_f32(&mut buf, f.offset + 4, get(1));
                    put_f32(&mut buf, f.offset + 8, get(2));
                }
                FieldTy::Vec4 => {
                    put_f32(&mut buf, f.offset, get(0));
                    put_f32(&mut buf, f.offset + 4, get(1));
                    put_f32(&mut buf, f.offset + 8, get(2));
                    put_f32(&mut buf, f.offset + 12, get(3));
                }
            }
        }
        buf
    }

    /// Advance every generator one frame.
    ///
    /// A generator is a float the shader reads but the user never sets:
    /// `time_base` integrates a speed, the filters follow another input. Each
    /// parameter is either a literal or the name of an input to read it from.
    ///
    /// Integrated per frame rather than derived from TIME for the same reason
    /// `PHASE_TIME_*` is: `TIME * speed` jumps when the speed changes, an
    /// accumulator carries on smoothly from where it was.
    ///
    /// ponytail: the filters (`damper`, `adsr`, `linear_filter`, `ease_filter`)
    /// pass their source straight through — the value they settle on, without
    /// the smoothing on the way. Upgrade path is a per-kind step function here;
    /// they are 13 of the 850 generators in MadMapper's own corpus.
    fn advance_generators(&mut self, delta: f32, engine: &EngineState, state: &IsfState) {
        let Some(manifest) = self.manifest.as_ref() else {
            return;
        };
        let mut acc = std::mem::take(&mut self.generators);
        for (i, g) in manifest.generators.iter().enumerate() {
            let value = |key: &str, default: f32| match g.params.get(key) {
                Some(serde_json::Value::String(name)) => engine
                    .get_param(name)
                    .or_else(|| state.values.get(name).copied())
                    .unwrap_or(default),
                Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(default as f64) as f32,
                Some(serde_json::Value::Bool(b)) => f32::from(u8::from(*b)),
                _ => default,
            };
            let rate = delta * value("speed", 1.0) * if value("reverse", 0.0) != 0.0 { -1.0 } else { 1.0 };
            acc[i] = match g.ty.as_str() {
                "time_base" => acc[i] + rate,
                // An animator is a time base wrapped into the 0..1 its shapes
                // are defined over. The shape itself is left linear.
                "animator" => (acc[i] + rate).rem_euclid(1.0),
                "multiplier" => (1..=4).map(|n| value(&format!("value{n}"), 1.0)).product(),
                "damper" | "adsr" | "linear_filter" | "ease_filter" | "shaper" | "curve"
                | "pass_thru" => value("input_value", 0.0),
                _ => 0.0,
            };
        }
        self.generators = acc;
    }

    /// std140-pack the IsfData block (64 bytes).
    fn pack_data(&mut self, engine: &EngineState, state: &IsfState) -> [u8; 64] {
        let now = Instant::now();
        let delta = self.fixed_delta.unwrap_or_else(|| {
            self.last_frame
                .map(|t| now.duration_since(t).as_secs_f32())
                .unwrap_or(0.0)
        });
        self.last_frame = Some(now);
        let frame = self.frame_index;
        self.frame_index += 1;

        // Restart is a trigger: the UI toggles it and the edge lands here,
        // since a `&EngineState` cannot clear a flag it was handed.
        let reset = engine.get_param(TIME_RESET).unwrap_or(0.0);
        if reset != self.last_reset {
            self.last_reset = reset;
            self.time = 0.0;
            self.phase = [0.0; 4];
            self.generators.fill(0.0);
        }

        // The instance clock. Speed scales it; Sync pins one shader-second to
        // the chosen beat division, so a shader that loops on `mod(TIME, 1.0)`
        // comes round on the beat grid. A shader whose loop is some other
        // length — `sin(TIME)` is 2π long — is dialled in with Speed.
        //
        // ponytail: rate-locked, not phase-locked — nothing snaps TIME to the
        // downbeat, so the loop keeps whatever offset it started with. Add a
        // wrap-snap like `Lfo::update` if it drifts off the beat audibly.
        let rate = clock_rate(
            engine.get_param(TIME_SPEED).unwrap_or(1.0),
            engine.get_param(TIME_SYNC).unwrap_or(0.0) >= 0.5,
            engine.get_param(TIME_DIV).unwrap_or(0.0) as usize,
            engine.effective_bpm(),
        );
        // Everything the instance drives off time runs on this one clock: the
        // TIME built-in, the phase accumulators, and the MadMapper generators.
        let delta = delta * rate;
        self.time += delta;

        // Integrate the phase accumulators for this frame. `get_param` resolves
        // against the effect's active prefix, so the driving parameter is found
        // per instance — two layers running the same shader keep their own phase.
        for pi in &self.phase_inputs {
            let rate = engine.get_param(&pi.param).unwrap_or(1.0);
            self.phase[pi.index] += delta * rate * pi.scale;
        }
        self.advance_generators(delta, engine, state);

        let mut buf = [0u8; 64];
        put_i32(&mut buf, 0, 0); // PASSINDEX (multipass = follow-up)
        let [width, height] = self.offscreen_size.unwrap_or([
            engine.resolution.internal_width,
            engine.resolution.internal_height,
        ]);
        put_f32(&mut buf, 8, width as f32);
        put_f32(&mut buf, 12, height as f32);
        put_f32(&mut buf, 16, self.time); // TIME
        put_f32(&mut buf, 20, delta); // TIMEDELTA
        let (y, mo, d, s) = current_date();
        put_f32(&mut buf, 32, y);
        put_f32(&mut buf, 36, mo);
        put_f32(&mut buf, 40, d);
        put_f32(&mut buf, 44, s); // DATE = (year, month, day, seconds since midnight)
        put_i32(&mut buf, 48, frame as i32); // FRAMEINDEX
        buf
    }
}

// ---------------------------------------------------------------------------
// Pass size expressions
// ---------------------------------------------------------------------------

/// Evaluate an ISF pass `WIDTH`/`HEIGHT` expression against the render size.
///
/// The grammar is what the corpus actually uses and nothing more: numbers,
/// `$WIDTH` / `$HEIGHT`, `$someInput` (looked up as a parameter), the four
/// arithmetic operators, parentheses, and `floor` / `min` / `max`. Anything
/// else returns `None`, and the target falls back to the render size.
fn eval_size_expr(expr: &str, size: [u32; 2], lookup: &dyn Fn(&str) -> Option<f32>) -> Option<f32> {
    let mut ev = SizeExpr {
        b: expr.as_bytes(),
        i: 0,
        size,
        lookup,
    };
    let v = ev.expr()?;
    ev.space();
    (ev.i == ev.b.len() && v.is_finite()).then_some(v)
}

struct SizeExpr<'a> {
    b: &'a [u8],
    i: usize,
    size: [u32; 2],
    lookup: &'a dyn Fn(&str) -> Option<f32>,
}

impl<'a> SizeExpr<'a> {
    fn space(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.space();
        let hit = self.b.get(self.i) == Some(&c);
        self.i += usize::from(hit);
        hit
    }
    fn expr(&mut self) -> Option<f32> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f32> {
        let mut v = self.unary()?;
        loop {
            if self.eat(b'*') {
                v *= self.unary()?;
            } else if self.eat(b'/') {
                v /= self.unary()?;
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f32> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        self.primary()
    }
    fn primary(&mut self) -> Option<f32> {
        self.space();
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        match self.b.get(self.i)? {
            b'$' => {
                self.i += 1;
                let name = self.ident();
                match name {
                    "WIDTH" => Some(self.size[0] as f32),
                    "HEIGHT" => Some(self.size[1] as f32),
                    _ => (self.lookup)(name),
                }
            }
            c if c.is_ascii_digit() || *c == b'.' => {
                let start = self.i;
                while self
                    .b
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok()
            }
            _ => {
                let name = self.ident();
                if name.is_empty() || !self.eat(b'(') {
                    return None;
                }
                let a = self.expr()?;
                let b = self.eat(b',').then(|| self.expr()).flatten();
                if !self.eat(b')') {
                    return None;
                }
                match (name, b) {
                    ("floor", None) => Some(a.floor()),
                    ("ceil", None) => Some(a.ceil()),
                    ("min", Some(b)) => Some(a.min(b)),
                    ("max", Some(b)) => Some(a.max(b)),
                    _ => None,
                }
            }
        }
    }
    fn ident(&mut self) -> &'a str {
        let start = self.i;
        while self
            .b
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// std140 write helpers
// ---------------------------------------------------------------------------

fn put_f32(buf: &mut [u8], off: usize, v: f32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// (year, month, day, seconds since midnight) from system time — civil-from-days
/// (Howard Hinnant's algorithm), no deps.
fn current_date() -> (f32, f32, f32, f32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    (y as f32, m as f32, d as f32, sod as f32)
}

// ---------------------------------------------------------------------------
// Vertex shader generation
// ---------------------------------------------------------------------------

/// One user IO channel of the fragment entry point: `@location(n)` + WGSL type.
struct FragInput {
    location: u32,
    wgsl_ty: String,
    flat: bool,
}

/// Inspect the compiled fragment WGSL and return its `@location` inputs.
/// The vertex stage must provide exactly these (wgpu validates input/output matching).
fn fragment_inputs(wgsl: &str, frag_entry: &str) -> Vec<FragInput> {
    let Ok(module) = naga::front::wgsl::parse_str(wgsl) else {
        return Vec::new();
    };
    let Some(ep) = module
        .entry_points
        .iter()
        .find(|e| e.stage == naga::ShaderStage::Fragment && e.name == frag_entry)
    else {
        return Vec::new();
    };
    let mut inputs: Vec<FragInput> = Vec::new();
    let mut push = |binding: &naga::Binding, ty: naga::Handle<naga::Type>| {
        if let naga::Binding::Location {
            location,
            interpolation,
            ..
        } = binding
        {
            inputs.push(FragInput {
                location: *location,
                wgsl_ty: wgsl_type(&module, ty),
                flat: *interpolation == Some(naga::Interpolation::Flat),
            });
        }
    };
    for arg in &ep.function.arguments {
        match (&arg.binding, &module.types[arg.ty].inner) {
            (Some(b), _) => push(b, arg.ty),
            // naga sometimes bundles IO into a struct argument
            (None, naga::TypeInner::Struct { members, .. }) => {
                for m in members {
                    if let Some(b) = &m.binding {
                        push(b, m.ty);
                    }
                }
            }
            _ => {}
        }
    }
    inputs.sort_by_key(|i| i.location);
    inputs
}

/// Render a naga type as WGSL text (scalar/vector float/int/uint; defaults vec2<f32>).
fn wgsl_type(module: &naga::Module, ty: naga::Handle<naga::Type>) -> String {
    use naga::{ScalarKind, TypeInner};
    let scalar = |kind: ScalarKind| match kind {
        ScalarKind::Sint => "i32",
        ScalarKind::Uint => "u32",
        _ => "f32",
    };
    match &module.types[ty].inner {
        TypeInner::Scalar(s) => scalar(s.kind).to_string(),
        TypeInner::Vector { size, scalar: s } => {
            let n = match size {
                naga::VectorSize::Bi => 2,
                naga::VectorSize::Tri => 3,
                naga::VectorSize::Quad => 4,
            };
            format!("vec{n}<{}>", scalar(s.kind))
        }
        _ => "vec2<f32>".to_string(),
    }
}

fn zero_const(wgsl_ty: &str) -> String {
    match wgsl_ty {
        "f32" => "0.0".to_string(),
        "i32" => "0i".to_string(),
        "u32" => "0u".to_string(),
        t if t.starts_with("vec") => format!("{t}()"),
        _ => "vec2<f32>()".to_string(),
    }
}

/// Generate our own tiny vertex module: fullscreen quad in,
/// `isf_FragNormCoord` at location 0, plus zero-valued
/// outputs for any extra fragment inputs (convolution shaders declare per-vertex
/// texOffsets varyings — real ISF hosts compute them vertex-side; zeros keep the
/// pipeline valid, rendering is approximate. ponytail: proper vertex-side offset
/// computation is a follow-up).
///
/// `flip_y` delivers the coordinate in ISF's bottom-left convention. That is
/// only correct for shaders whose sampling goes through the `IMG_*` rewrite,
/// which flips it back; a shader that samples directly would be inverted once
/// per pass. See [`IsfManifest::flip_frag_norm_coord`].
fn generate_vertex_wgsl(frag_inputs: &[FragInput], flip_y: bool) -> String {
    let mut fields = String::new();
    let mut assigns = String::new();
    for fi in frag_inputs {
        let interp = if fi.flat { " @interpolate(flat)" } else { "" };
        fields.push_str(&format!(
            "    @location({}){interp} o{}: {},\n",
            fi.location, fi.location, fi.wgsl_ty
        ));
        let value = if fi.location == 0 {
            if flip_y {
                "vec2<f32>(in.uv.x, 1.0 - in.uv.y)".to_string()
            } else {
                "in.uv".to_string()
            }
        } else {
            zero_const(&fi.wgsl_ty)
        };
        assigns.push_str(&format!("    out.o{} = {value};\n", fi.location));
    }
    format!(
        "struct VsIn {{\n    @location(0) pos: vec2<f32>,\n    @location(1) uv: vec2<f32>,\n}};\n\
         struct VsOut {{\n    @builtin(position) pos: vec4<f32>,\n{fields}}};\n\
         @vertex\nfn vs_main(in: VsIn) -> VsOut {{\n    var out: VsOut;\n    out.pos = vec4<f32>(in.pos, 0.0, 1.0);\n{assigns}    return out;\n}}\n"
    )
}

// ---------------------------------------------------------------------------
// EffectPlugin
// ---------------------------------------------------------------------------

impl EffectPlugin for IsfEffect {
    type State = IsfState;
    type Uniforms = IsfUniforms;

    fn app_name(&self) -> &str {
        "isf-example"
    }

    fn parameters_dirty(&self) -> bool {
        self.params_dirty
    }
    fn clear_parameters_dirty(&mut self) {
        self.params_dirty = false;
    }

    fn shader_source(&self) -> &'static str {
        // The engine compiles this stub, but render() returns true so it is never used.
        include_str!("shaders/passthrough.wgsl")
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut params = if self.uses_clock {
            time_parameters()
        } else {
            Vec::new()
        };
        params.extend(isf_inputs_to_parameters(&self.isf.inputs));
        params
    }

    fn default_state(&self) -> IsfState {
        IsfState {
            values: crate::params::isf_inputs_to_default_values(&self.isf.inputs),
        }
    }

    fn build_uniforms(&self, _state: &IsfState, _engine: &EngineState) -> IsfUniforms {
        // Vestigial: real uniforms are std140-packed per frame in render().
        IsfUniforms([0.0; MAX_ISF_UNIFORMS])
    }

    // -----------------------------------------------------------------------
    // Hot reload — called every frame via prepare()
    // -----------------------------------------------------------------------

    fn prepare(
        &mut self,
        _app_state: &mut IsfState,
        _engine: &EngineState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        // Check for a new path requested via the "Load Shader" button.
        if let Ok(mut guard) = self.pending_path.lock()
            && let Some(new_path) = guard.take() {
                self.shader_path = resolve_shader_path(&new_path);
                // Derive and broadcast the new display name immediately.
                let name = self
                    .shader_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("ISF Shader")
                    .to_string();
                self.shader_name = name.clone();
                if let Ok(mut shared) = self.shader_name_shared.lock() {
                    *shared = name;
                }
                self.last_mtime = None; // force reload below
            }

        let Ok(meta) = std::fs::metadata(&self.shader_path) else {
            return;
        };
        let Ok(mtime) = meta.modified() else { return };
        // A companion `.vs` is part of the shader, so editing it reloads too.
        let mtime = std::fs::metadata(self.shader_path.with_extension("vs"))
            .and_then(|m| m.modified())
            .map_or(mtime, |vs| vs.max(mtime));
        if self.last_mtime == Some(mtime) {
            return;
        }
        self.last_mtime = Some(mtime);

        let src = match std::fs::read_to_string(&self.shader_path) {
            Ok(s) => s,
            Err(e) => {
                self.transpile_error = Some(format!("Read error: {e}"));
                return;
            }
        };
        match crate::header::parse(&src) {
            Ok(isf) => {
                self.isf = isf;
                self.glsl_src = src;
                // Everything derived from the source, or an edit that adds a
                // PHASE_INPUTS or a TIME would not take until the next launch.
                self.phase_inputs = parse_phase_inputs(&self.glsl_src);
                self.uses_clock = uses_clock(&self.glsl_src);
            }
            Err(e) => {
                self.transpile_error = Some(format!("ISF parse error: {e}"));
                return;
            }
        }
        log::info!("Hot-reloading shader: {}", self.shader_path.display());
        self.init(device, queue);
    }

    // -----------------------------------------------------------------------
    // Init — compile ISF pipeline (dynamic BGL from the manifest)
    // -----------------------------------------------------------------------

    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        let transpiled = match compile::generate_wgsl(&self.isf, &self.glsl_src) {
            Ok(t) => t,
            Err(e) => {
                self.transpile_error = Some(format!("Transpile error: {}", e));
                log::error!("ISF transpile error: {}", e);
                return;
            }
        };
        let manifest = transpiled.manifest;

        log::debug!(
            "ISF: Generated WGSL for {}:\n{}",
            self.shader_name,
            transpiled.wgsl
        );

        // Compile shaders — wgpu panics on WGSL validation errors; catch_unwind prevents crash.
        // A companion `.vs` beside the shader is its own vertex stage; anything
        // wrong with it falls back to the generated one rather than failing the
        // shader, since the generated stage is what every other shader uses.
        let companion = std::fs::read_to_string(self.shader_path.with_extension("vs"))
            .ok()
            .map(|src| compile::compile_vertex(&src, &manifest));
        let vertex_wgsl = match &companion {
            Some(Ok(wgsl)) => wgsl.clone(),
            other => {
                if let Some(Err(e)) = other {
                    log::info!(
                        "ISF: ignoring companion .vs for {}: {e}",
                        self.shader_name
                    );
                }
                generate_vertex_wgsl(
                    &fragment_inputs(&transpiled.wgsl, &manifest.frag_entry),
                    manifest.flip_frag_norm_coord,
                )
            }
        };
        // Its stage reads the uniform blocks, so they have to be visible to it.
        let stages = if matches!(companion, Some(Ok(_))) {
            wgpu::ShaderStages::VERTEX_FRAGMENT
        } else {
            wgpu::ShaderStages::FRAGMENT
        };
        // glslang names the companion stage's entry `main`; the generated one is
        // `vs_main`.
        let vertex_entry = naga::front::wgsl::parse_str(&vertex_wgsl)
            .ok()
            .and_then(|m| {
                m.entry_points
                    .iter()
                    .find(|ep| ep.stage == naga::ShaderStage::Vertex)
                    .map(|ep| ep.name.clone())
            })
            .unwrap_or_else(|| "vs_main".to_string());
        let shader_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let frag = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ISF Fragment Shader"),
                source: wgpu::ShaderSource::Wgsl(transpiled.wgsl.clone().into()),
            });
            let vert = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ISF Vertex Shader"),
                source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
            });
            (frag, vert)
        }));
        let (frag_shader, vert_shader) = match shader_result {
            Ok(s) => s,
            Err(_) => {
                self.transpile_error = Some(
                    "WGSL compilation failed (shader may use unsupported GLSL features like function overloading)"
                        .to_string(),
                );
                log::error!("ISF: WGSL compilation panicked for {}", self.shader_name);
                return;
            }
        };

        // Dynamic bind group layout from the manifest.
        let mut bgl_entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: stages,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        if manifest.inputs_block_size > 0 {
            bgl_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: stages,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        if manifest.has_sampler {
            bgl_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: stages,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        for t in &manifest.textures {
            bgl_entries.push(wgpu::BindGroupLayoutEntry {
                binding: t.binding,
                visibility: stages,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ISF BGL"),
            entries: &bgl_entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ISF Pipeline Layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ISF Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vert_shader,
                entry_point: Some(&vertex_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(Vertex::desc())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &frag_shader,
                entry_point: Some(&manifest.frag_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self
                        .offscreen_format
                        .unwrap_or_else(rustjay_core::working_format),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Fullscreen quad
        let vertices = Vertex::quad_vertices();
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ISF Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Uniform buffers — one IsfData per pass (they differ only in PASSINDEX,
        // but all of a frame's queue writes land before any of its passes run).
        let data_buffers: Vec<wgpu::Buffer> = (0..self.isf.passes.len().max(1))
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("ISF IsfData Buffer {i}")[..]),
                    size: 64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let inputs_buffer = (manifest.inputs_block_size > 0).then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ISF IsfInputs Buffer"),
                size: manifest.inputs_block_size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        // 1×1 black placeholder texture + our own filtering sampler
        let placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ISF Placeholder Texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let placeholder_view = placeholder.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ISF Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Precompute per-field lookup keys (no per-frame allocation).
        let generators = manifest.generators.clone();
        self.generators = vec![0.0; generators.len()];
        self.pack_fields = manifest
            .input_fields
            .iter()
            .map(|f| {
                let k = match f.ty {
                    FieldTy::Vec2 | FieldTy::Vec3 => [
                        format!("{}_x", f.name),
                        format!("{}_y", f.name),
                        format!("{}_z", f.name),
                        String::new(),
                    ],
                    FieldTy::Vec4 => [
                        format!("{}_r", f.name),
                        format!("{}_g", f.name),
                        format!("{}_b", f.name),
                        format!("{}_a", f.name),
                    ],
                    _ => [f.name.clone(), String::new(), String::new(), String::new()],
                };
                PackField {
                    offset: f.offset,
                    ty: f.ty,
                    k,
                    phase: f
                        .name
                        .strip_prefix("PHASE_TIME_")
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|n| *n < 4),
                    generator: generators.iter().position(|g| g.name == f.name),
                }
            })
            .collect();

        // Primary texture input: "inputImage" when present, else first image/audio
        // input. A laser material has neither — what it reads is its own previous
        // frame, which the host passes in the same way an effect gets its input.
        self.primary_texture = if manifest.laser.is_some() {
            manifest
                .textures
                .iter()
                .find(|t| t.name == crate::compile::LAST_FRAME_DATA)
                .map(|t| t.name.clone())
        } else {
            self.isf
                .inputs
                .iter()
                .find(|i| i.name == "inputImage")
                .or_else(|| {
                    self.isf.inputs.iter().find(|i| {
                        matches!(
                            i.ty,
                            isf::InputType::Image
                                | isf::InputType::Audio(_)
                                | isf::InputType::AudioFft(_)
                        )
                    })
                })
                .map(|i| i.name.clone())
        };

        // The first image input that is not the primary — declaration order.
        self.secondary_texture = self
            .isf
            .inputs
            .iter()
            .filter(|i| matches!(i.ty, isf::InputType::Image))
            .map(|i| i.name.clone())
            .find(|name| Some(name) != self.primary_texture.as_ref());

        self.pipeline = Some(pipeline);
        self.bind_group_layout = Some(bgl);
        self.vertex_buffer = Some(vb);
        // A last pass with a TARGET still has to reach the screen — see `blit`.
        self.blit = self
            .isf
            .passes
            .last()
            .is_some_and(|p| p.target.is_some())
            .then(|| {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("ISF Blit Shader"),
                    source: wgpu::ShaderSource::Wgsl(
                        include_str!("shaders/passthrough.wgsl").into(),
                    ),
                });
                let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("ISF Blit BGL"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("ISF Blit Pipeline Layout"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                });
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("ISF Blit Pipeline"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(Vertex::desc())],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: self
                                .offscreen_format
                                .unwrap_or_else(rustjay_core::working_format),
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
                (pipeline, bgl)
            });
        self.data_buffers = data_buffers;
        // Dropped so the next render reallocates against the new header.
        self.pass_targets.clear();
        self.inputs_buffer = inputs_buffer;
        self.placeholder_view = Some(placeholder_view);
        self.sampler = Some(sampler);
        self.manifest = Some(manifest);
        self.transpile_error = None;
        self.params_dirty = true;

        // Persist the current shader path so the next launch starts from here.
        let config = super::last_shader_config_path();
        if let Some(parent) = config.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&config, self.shader_path.to_string_lossy().as_bytes());
    }

    // -----------------------------------------------------------------------
    // Custom render
    // -----------------------------------------------------------------------

    fn render(
        &mut self,
        ctx: &mut rustjay_core::RenderHookCtx<'_>,
        app_state: &mut IsfState,
    ) -> bool {
        if self.pipeline.is_none()
            || self.vertex_buffer.is_none()
            || self.data_buffers.is_empty()
            || self.bind_group_layout.is_none()
            || self.manifest.is_none()
        {
            return true; // pipeline not ready — render black
        }

        let size = self.offscreen_size.unwrap_or([
            ctx.engine_state.resolution.internal_width,
            ctx.engine_state.resolution.internal_height,
        ]);
        // `$someInput` in a pass size expression resolves like any parameter.
        let lookup = |name: &str| {
            ctx.engine_state
                .get_param(name)
                .or_else(|| app_state.values.get(name).copied())
        };
        self.ensure_pass_targets(ctx.device, size, &lookup);

        // Upload uniforms (std140-packed)
        let mut data = self.pack_data(ctx.engine_state, app_state);
        let inputs = self.pack_inputs(app_state, ctx.engine_state);
        let pipeline = self.pipeline.as_ref().unwrap();
        let vb = self.vertex_buffer.as_ref().unwrap();
        let bgl = self.bind_group_layout.as_ref().unwrap();
        let manifest = self.manifest.as_ref().unwrap();
        for (i, buf) in self.data_buffers.iter().enumerate() {
            put_i32(&mut data, 0, i as i32); // PASSINDEX
            // RENDERSIZE is the pass's own target, which is what a shader
            // sampling a quarter-res buffer by pixel coordinate expects.
            let pass_size = self
                .isf
                .passes
                .get(i)
                .and_then(|p| p.target.as_deref())
                .and_then(|name| self.pass_targets.iter().find(|t| t.name == name))
                .map_or(size, |t| t.size);
            put_f32(&mut data, 8, pass_size[0] as f32);
            put_f32(&mut data, 12, pass_size[1] as f32);
            ctx.queue.write_buffer(buf, 0, &data);
        }
        if let Some(inputs_buf) = &self.inputs_buffer {
            ctx.queue.write_buffer(inputs_buf, 0, &inputs);
        }

        let ping = self.ping;
        // One draw per PASSES entry (a shader with no PASSES is one pass to
        // the engine's target).
        for (i, data_buf) in self.data_buffers.iter().enumerate() {
            let pass_def = self.isf.passes.get(i);
            let own_target = pass_def.and_then(|p| p.target.as_deref());

            // Build the set-0 bind group fresh each pass (views change per pass).
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: data_buf.as_entire_binding(),
            }];
            if let Some(inputs_buf) = &self.inputs_buffer {
                entries.push(wgpu::BindGroupEntry {
                    binding: 1,
                    resource: inputs_buf.as_entire_binding(),
                });
            }
            if manifest.has_sampler {
                entries.push(wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(self.sampler.as_ref().unwrap()),
                });
            }
            for t in &manifest.textures {
                let placeholder = self.placeholder_view.as_ref().unwrap();
                let view = match self.pass_targets.iter().find(|p| p.name == t.name) {
                    // A temporary cannot be sampled by the pass that renders
                    // into it — that is a read of its own attachment — so that
                    // one pass sees black. Make it PERSISTENT to read it back.
                    Some(target) if own_target == Some(t.name.as_str()) && !target.persistent => {
                        placeholder
                    }
                    Some(target) => target.read_view(ping),
                    // The second image input takes the host's second texture
                    // when there is one — that is what makes a transition
                    // actually transition rather than blend a frame with
                    // itself.
                    None if ctx.input_b.is_some()
                        && self.secondary_texture.as_deref() == Some(t.name.as_str()) =>
                    {
                        ctx.input_b.as_ref().unwrap().view
                    }
                    // Otherwise the engine has one video input, so every image
                    // input of a shader gets it — a two-input shader (a
                    // datamosh driven by a `motionImage`, a transition) is
                    // otherwise dead in the water with a black second input.
                    // Audio inputs, which have no frame to give them, still
                    // sample black.
                    None => match (&ctx.input, &self.primary_texture) {
                        (Some(input), Some(primary))
                            if *primary == t.name || self.is_image_input(&t.name) =>
                        {
                            input.view
                        }
                        _ => placeholder,
                    },
                };
                entries.push(wgpu::BindGroupEntry {
                    binding: t.binding,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
            let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ISF Bind Group"),
                layout: bgl,
                entries: &entries,
            });

            // A pass with a TARGET renders into it; the one without (the last,
            // by ISF convention) is what reaches the engine.
            let target_view = own_target
                .and_then(|name| self.pass_targets.iter().find(|p| p.name == name))
                .map_or(ctx.target_view, |t| t.write_view(ping));

            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ISF Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        if let Some((blit_pipeline, blit_bgl)) = &self.blit
            && let Some(target) = self.pass_targets.last()
        {
            let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ISF Blit Bind Group"),
                layout: blit_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(target.write_view(ping)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(self.sampler.as_ref().unwrap()),
                    },
                ],
            });
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ISF Blit Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(blit_pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        self.ping ^= 1;

        true
    }
}

#[cfg(test)]
mod size_expr_tests {
    use super::eval_size_expr;

    fn ev(expr: &str) -> Option<f32> {
        eval_size_expr(expr, [1920, 1080], &|name| (name == "buffQuality").then_some(0.5))
    }

    #[test]
    fn the_forms_the_corpus_uses() {
        assert_eq!(ev("$WIDTH"), Some(1920.0));
        assert_eq!(ev("$HEIGHT/3.0"), Some(360.0));
        assert_eq!(ev("floor($WIDTH/4.0)"), Some(480.0));
        assert_eq!(ev("max(floor($HEIGHT*0.02),1.0)"), Some(21.0));
        assert_eq!(ev("floor($WIDTH*min((0.2),1.0))"), Some(384.0));
        assert_eq!(ev("$WIDTH / 100.0"), Some(19.2));
        assert_eq!(ev("64"), Some(64.0));
        assert_eq!(ev("floor($WIDTH*$buffQuality)"), Some(960.0));
    }

    #[test]
    fn anything_else_falls_back_to_the_render_size() {
        assert_eq!(ev("$WIDTH/"), None);
        assert_eq!(ev("clamp($WIDTH, 1.0, 2.0)"), None); // unsupported function
        assert_eq!(ev("$noSuchInput"), None);
        assert_eq!(ev("$WIDTH 4"), None); // trailing junk
        assert_eq!(ev("1.0/0.0"), None); // not finite
    }
}

#[cfg(test)]
mod clock_tests {
    use super::clock_rate;

    /// Division 4 is a whole note — four beats, a bar in 4/4. At 120 BPM that
    /// is 2 s, so TIME must advance at 0.5/s for a `mod(TIME, 1.0)` loop to
    /// come round exactly on the bar.
    #[test]
    fn a_bar_long_loop_at_120_bpm_runs_at_half_speed() {
        assert_eq!(clock_rate(1.0, true, 4, 120.0), 0.5);
    }

    #[test]
    fn speed_scales_the_synced_clock_too() {
        assert_eq!(clock_rate(2.0, true, 4, 120.0), 1.0);
        assert_eq!(clock_rate(2.0, false, 4, 120.0), 2.0);
    }

    /// Without a tempo there is nothing to lock to, so the clock free-runs at
    /// Speed instead of crawling towards zero.
    #[test]
    fn no_tempo_falls_back_to_free_running() {
        assert_eq!(clock_rate(1.0, true, 4, 0.0), 1.0);
    }

    /// A filter that only reshapes its input has no pacing to control.
    #[test]
    fn a_shader_without_a_clock_declares_no_pacing() {
        let src = "void main() { gl_FragColor = IMG_THIS_PIXEL(inputImage) * brightness; }";
        assert!(!super::uses_clock(src));
    }

    #[test]
    fn the_clock_is_found_however_a_shader_reaches_for_it() {
        for src in [
            "void main() { float t = mod(TIME, 1.0); }",
            "void main() { acc += TIMEDELTA; }",
            "/*{\"PHASE_INPUTS\": [{\"PARAM\": \"speed\"}]}*/",
            "/*{\"GENERATORS\": [{\"NAME\": \"t\", \"TYPE\": \"time_base\"}]}*/",
            "void main() { float x = PHASE_TIME_0; }",
        ] {
            assert!(super::uses_clock(src), "{src}");
        }
    }

    /// TIME inside a longer identifier is somebody's variable, not the builtin.
    #[test]
    fn a_word_containing_time_is_not_the_builtin() {
        assert!(!super::uses_clock("float RUNTIME_MAX = 1.0; float lifetime = 2.0;"));
    }

    /// An out-of-range division must not panic — the value arrives as an f32
    /// parameter and anything can write to it.
    #[test]
    fn an_out_of_range_division_clamps() {
        assert_eq!(clock_rate(1.0, true, 99, 120.0), clock_rate(1.0, true, 7, 120.0));
    }
}

#[cfg(test)]
mod phase_tests {
    use super::parse_phase_inputs;

    #[test]
    fn reads_phase_inputs_with_defaults() {
        let src = r#"/*{
            "INPUTS": [],
            "PHASE_INPUTS": [{"PARAM": "flow_speed", "INDEX": 0}]
        }*/
        void main() {}"#;
        let got = parse_phase_inputs(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].param, "flow_speed");
        assert_eq!(got[0].index, 0);
        assert_eq!(got[0].scale, 1.0, "SCALE defaults to 1");
    }

    #[test]
    fn honours_scale_and_index() {
        let src = r#"/*{"PHASE_INPUTS": [{"PARAM": "spin", "INDEX": 2, "SCALE": 0.5}]}*/"#;
        let got = parse_phase_inputs(src);
        assert_eq!(got[0].index, 2);
        assert_eq!(got[0].scale, 0.5);
    }

    /// Only four accumulators exist; a fifth would index out of bounds every
    /// frame, so it is dropped at parse time.
    #[test]
    fn drops_an_out_of_range_index() {
        let src = r#"/*{"PHASE_INPUTS": [{"PARAM": "a", "INDEX": 4}]}*/"#;
        assert!(parse_phase_inputs(src).is_empty());
    }

    #[test]
    fn a_shader_without_phase_inputs_gets_none() {
        let src = r#"/*{"INPUTS": [{"NAME": "x", "TYPE": "float"}]}*/ void main() {}"#;
        assert!(parse_phase_inputs(src).is_empty());
    }

    /// A licence block above the ISF header must not be mistaken for it.
    #[test]
    fn skips_a_leading_non_json_comment() {
        let src = r#"/* Copyright someone, all rights reserved. */
        /*{"PHASE_INPUTS": [{"PARAM": "rate", "INDEX": 1}]}*/
        void main() {}"#;
        let got = parse_phase_inputs(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].param, "rate");
    }
}
