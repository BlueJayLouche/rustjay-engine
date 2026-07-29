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
use rustjay_core::{EffectPlugin, EngineState, ParameterDescriptor, Vertex};
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

    /// Start time — used to compute elapsed seconds for the TIME built-in.
    start_time: Instant,
    /// Previous frame's timestamp — for TIMEDELTA.
    last_frame: Option<Instant>,
    /// FRAMEINDEX built-in counter.
    frame_index: u64,

    /// Error message from transpilation / compilation (shown in GUI).
    pub transpile_error: Option<String>,

    // GPU resources (created in init())
    pipeline: Option<wgpu::RenderPipeline>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    vertex_buffer: Option<wgpu::Buffer>,
    /// IsfData block (binding 0), always 64 bytes.
    data_buffer: Option<wgpu::Buffer>,
    /// IsfInputs block (binding 1), present when inputs_block_size > 0.
    inputs_buffer: Option<wgpu::Buffer>,
    /// 1×1 black placeholder for unbound texture inputs.
    placeholder_view: Option<wgpu::TextureView>,
    /// Our own filtering sampler (the GLSL constructs sampler2D(t, img_sampler)).
    sampler: Option<wgpu::Sampler>,

    manifest: Option<IsfManifest>,
    /// Precomputed (offset, type, lookup-keys) per IsfInputs field — avoids
    /// per-frame `format!` when reading params/state.
    pack_fields: Vec<PackField>,
    /// Texture input that receives the upstream frame: "inputImage" when present,
    /// else the first image/audio input. None for pure generators.
    primary_texture: Option<String>,
}

/// A std140 field with its precomputed state/param lookup keys.
struct PackField {
    offset: usize,
    ty: FieldTy,
    /// Component keys: scalar fields use only `k[0]`; vec2 uses `k[0..2]`
    /// (`name_x`, `name_y`); vec4 uses `k[0..4]` (`name_r.._a`).
    k: [String; 4],
}

impl IsfEffect {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let glsl_src = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path.display(), e))?;
        let isf = isf::parse(&glsl_src)
            .map_err(|e| anyhow::anyhow!("ISF parse error in {}: {}", path.display(), e))?;
        let shader_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ISF Shader")
            .to_string();

        Ok(Self {
            isf,
            glsl_src,
            shader_name_shared: Arc::new(Mutex::new(shader_name.clone())),
            shader_name,
            shader_path: path.to_path_buf(),
            last_mtime: std::fs::metadata(path).ok().and_then(|m| m.modified().ok()),
            pending_path: Arc::new(Mutex::new(None)),
            params_dirty: false,
            start_time: Instant::now(),
            last_frame: None,
            frame_index: 0,
            transpile_error: None,
            pipeline: None,
            bind_group_layout: None,
            vertex_buffer: None,
            data_buffer: None,
            inputs_buffer: None,
            placeholder_view: None,
            sampler: None,
            manifest: None,
            pack_fields: Vec::new(),
            primary_texture: None,
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

    /// std140-pack the IsfData block (64 bytes).
    fn pack_data(&mut self, engine: &EngineState) -> [u8; 64] {
        let now = Instant::now();
        let delta = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        let frame = self.frame_index;
        self.frame_index += 1;

        let mut buf = [0u8; 64];
        put_i32(&mut buf, 0, 0); // PASSINDEX (multipass = follow-up)
        put_f32(&mut buf, 8, engine.resolution.internal_width as f32);
        put_f32(&mut buf, 12, engine.resolution.internal_height as f32);
        put_f32(&mut buf, 16, self.start_time.elapsed().as_secs_f32()); // TIME
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

/// Generate our own tiny vertex module: fullscreen quad in, Y-flipped
/// `isf_FragNormCoord` (ISF bottom-left origin) at location 0, plus zero-valued
/// outputs for any extra fragment inputs (convolution shaders declare per-vertex
/// texOffsets varyings — real ISF hosts compute them vertex-side; zeros keep the
/// pipeline valid, rendering is approximate. ponytail: proper vertex-side offset
/// computation is a follow-up).
fn generate_vertex_wgsl(frag_inputs: &[FragInput]) -> String {
    let mut fields = String::new();
    let mut assigns = String::new();
    for fi in frag_inputs {
        let interp = if fi.flat { " @interpolate(flat)" } else { "" };
        fields.push_str(&format!(
            "    @location({}){interp} o{}: {},\n",
            fi.location, fi.location, fi.wgsl_ty
        ));
        let value = if fi.location == 0 {
            "vec2<f32>(in.uv.x, 1.0 - in.uv.y)".to_string()
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
        isf_inputs_to_parameters(&self.isf.inputs)
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
                self.shader_path = new_path;
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
        match isf::parse(&src) {
            Ok(isf) => {
                self.isf = isf;
                self.glsl_src = src;
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
        let vertex_wgsl = generate_vertex_wgsl(&fragment_inputs(&transpiled.wgsl, &manifest.frag_entry));
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
            visibility: wgpu::ShaderStages::FRAGMENT,
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
                visibility: wgpu::ShaderStages::FRAGMENT,
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
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        for t in &manifest.textures {
            bgl_entries.push(wgpu::BindGroupLayoutEntry {
                binding: t.binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
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
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &frag_shader,
                entry_point: Some(&manifest.frag_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: rustjay_core::working_format(),
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

        // Uniform buffers
        let data_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ISF IsfData Buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
                }
            })
            .collect();

        // Primary texture input: "inputImage" when present, else first image/audio input.
        self.primary_texture = self
            .isf
            .inputs
            .iter()
            .find(|i| i.name == "inputImage")
            .or_else(|| {
                self.isf.inputs.iter().find(|i| {
                    matches!(
                        i.ty,
                        isf::InputType::Image | isf::InputType::Audio(_) | isf::InputType::AudioFft(_)
                    )
                })
            })
            .map(|i| i.name.clone());

        self.pipeline = Some(pipeline);
        self.bind_group_layout = Some(bgl);
        self.vertex_buffer = Some(vb);
        self.data_buffer = Some(data_buffer);
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
            || self.data_buffer.is_none()
            || self.bind_group_layout.is_none()
            || self.manifest.is_none()
        {
            return true; // pipeline not ready — render black
        }

        // Upload uniforms (std140-packed)
        let data = self.pack_data(ctx.engine_state);
        let inputs = self.pack_inputs(app_state, ctx.engine_state);
        let pipeline = self.pipeline.as_ref().unwrap();
        let vb = self.vertex_buffer.as_ref().unwrap();
        let data_buf = self.data_buffer.as_ref().unwrap();
        let bgl = self.bind_group_layout.as_ref().unwrap();
        let manifest = self.manifest.as_ref().unwrap();
        ctx.queue.write_buffer(data_buf, 0, &data);
        if let Some(inputs_buf) = &self.inputs_buffer {
            ctx.queue.write_buffer(inputs_buf, 0, &inputs);
        }

        // Build the set-0 bind group fresh each frame (texture views may change).
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
            // Filters with no upstream texture sample black (ISF host behavior).
            let view = match (&ctx.input, &self.primary_texture) {
                (Some(input), Some(primary)) if *primary == t.name => input.view,
                _ => self.placeholder_view.as_ref().unwrap(),
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

        {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ISF Render Pass"),
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
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        true
    }
}
