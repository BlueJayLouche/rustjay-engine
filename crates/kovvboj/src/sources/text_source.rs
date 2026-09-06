//! Text source — rasterises a string with `ab_glyph` and blits it.
//!
//! The whole string is rasterised into one texture, and only when something
//! about the *layout* changes: the text, the font, tracking, line height or
//! alignment. Size, position, rotation and colour are blit-time uniforms, so
//! they cost nothing per frame and modulate smoothly.
//!
//! ponytail: no glyph atlas. One string per layer, re-rasterised on a cue, is
//! not worth packing and caching glyphs for. An atlas earns its keep when each
//! letter needs its own transform — that is a mesh path, not this one.

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use rustjay_core::{
    EffectInput, EffectInstance, EngineState, ParamCategory, ParameterDescriptor, RenderCtx,
    RenderTarget,
};
use std::path::{Path, PathBuf};

/// Cap height the string is rasterised at. Size is a blit-time scale, so this
/// only sets how far text can be blown up before it softens.
const RASTER_PX: f32 = 220.0;
/// Widest raster we will allocate. A long string is rasterised smaller rather
/// than refused — it is scaled at blit time anyway.
const MAX_RASTER_W: u32 = 4096;
/// Breathing room around the glyphs, for overhang the advance does not cover.
const PAD: u32 = 8;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    color: [f32; 4],
    // Not `target`: that is a reserved word in WGSL, and the shader will not
    // parse with a field of that name.
    resolution: [f32; 2],
    center: [f32; 2],
    scale: f32,
    tex_aspect: f32,
    angle: f32,
    _pad: f32,
}

/// What the raster depends on. Anything else is a blit-time uniform.
#[derive(Clone, PartialEq)]
struct Layout {
    text: String,
    tracking: f32,
    line_height: f32,
    align: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            text: "TEXT".to_string(),
            tracking: 0.0,
            line_height: 1.2,
            align: 1,
        }
    }
}

/// Renders a rasterised string, scaled and positioned on the target.
pub struct TextSource {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,

    font: Option<FontVec>,
    font_path: Option<PathBuf>,
    /// The layout the current raster was built from; a difference re-rasterises.
    layout: Layout,
    /// What the next raster should use. Set by the UI, OSC, or a parameter.
    pending: Layout,
    tex_size: [u32; 2],
    dirty: bool,

    param_prefix: String,
}

impl TextSource {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        font: Option<&Path>,
        text: Option<&str>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Text Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Text Uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Text BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Text Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Text Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(rustjay_core::Vertex::desc())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Text Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let font_path = font.map(Path::to_path_buf).or_else(default_font);
        let font = font_path.as_deref().and_then(load_font);
        let layout = Layout {
            text: text.unwrap_or("TEXT").to_string(),
            ..Layout::default()
        };

        Self {
            pipeline,
            bind_group_layout,
            bind_group: None,
            uniform_buffer,
            sampler,
            font,
            font_path,
            // Mismatched on purpose: the first `prepare` rasterises.
            layout: Layout {
                text: String::new(),
                ..layout.clone()
            },
            pending: layout,
            tex_size: [1, 1],
            dirty: true,
            param_prefix: String::new(),
        }
    }

    /// The string being rendered.
    pub fn text(&self) -> &str {
        &self.pending.text
    }

    /// Replace the string. Rasterises on the next `prepare`.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.pending.text = text.into();
    }

    /// The font file in use, if one was found.
    pub fn font_path(&self) -> Option<&Path> {
        self.font_path.as_deref()
    }

    /// Load a different font file. A file that will not parse is ignored, so a
    /// bad pick cannot blank a layer mid-set.
    pub fn set_font(&mut self, path: &Path) {
        if let Some(font) = load_font(path) {
            self.font = Some(font);
            self.font_path = Some(path.to_path_buf());
            self.dirty = true;
        } else {
            log::warn!("[Text] could not load font {}", path.display());
        }
    }

    /// Rasterise the pending layout into a fresh texture and bind group.
    fn rasterise(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(font) = &self.font else {
            return;
        };
        let (pixels, width, height) = raster(font, &self.pending);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Text Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Text BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniform_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        self.tex_size = [width, height];
        self.layout = self.pending.clone();
        self.dirty = false;
    }

    fn param(&self, engine: &EngineState, name: &str, default: f32) -> f32 {
        engine
            .get_param(&format!("{}{name}", self.param_prefix))
            .unwrap_or(default)
    }
}

impl EffectInstance for TextSource {
    fn label(&self) -> &str {
        "text"
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        // Bare names — the mixer applies the channel prefix.
        let cat = ParamCategory::Custom("Text".to_string());
        vec![
            ParameterDescriptor::float("text_size", "Size", cat.clone(), 0.0, 2.0, 0.4, 0.005),
            ParameterDescriptor::float("text_x", "X", cat.clone(), -1.0, 2.0, 0.5, 0.005),
            ParameterDescriptor::float("text_y", "Y", cat.clone(), -1.0, 2.0, 0.5, 0.005),
            ParameterDescriptor::float("text_rot", "Rotation", cat.clone(), -180.0, 180.0, 0.0, 1.0),
            ParameterDescriptor::float("text_r", "Red", cat.clone(), 0.0, 1.0, 1.0, 0.01),
            ParameterDescriptor::float("text_g", "Green", cat.clone(), 0.0, 1.0, 1.0, 0.01),
            ParameterDescriptor::float("text_b", "Blue", cat.clone(), 0.0, 1.0, 1.0, 0.01),
            ParameterDescriptor::float("text_a", "Alpha", cat.clone(), 0.0, 1.0, 1.0, 0.01),
            // Layout: a change here re-rasterises, so these are not the ones to
            // hang an LFO on.
            ParameterDescriptor::float("text_track", "Tracking", cat.clone(), -0.3, 1.0, 0.0, 0.01),
            ParameterDescriptor::float("text_line", "Line Height", cat.clone(), 0.5, 3.0, 1.2, 0.01),
            ParameterDescriptor::enum_param(
                "text_align",
                "Align",
                cat,
                vec!["Left".into(), "Centre".into(), "Right".into()],
                1,
            ),
        ]
    }

    fn prepare(&mut self, engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.pending.tracking = self.param(engine, "text_track", 0.0);
        self.pending.line_height = self.param(engine, "text_line", 1.2).max(0.1);
        self.pending.align = self.param(engine, "text_align", 1.0).round().clamp(0.0, 2.0) as usize;
        if self.dirty || self.pending != self.layout {
            self.rasterise(device, queue);
        }
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        engine: &EngineState,
    ) {
        let Some(bind_group) = &self.bind_group else {
            return;
        };
        let uniforms = Uniforms {
            color: [
                self.param(engine, "text_r", 1.0),
                self.param(engine, "text_g", 1.0),
                self.param(engine, "text_b", 1.0),
                self.param(engine, "text_a", 1.0),
            ],
            resolution: [target.size[0].max(1) as f32, target.size[1].max(1) as f32],
            center: [
                self.param(engine, "text_x", 0.5),
                self.param(engine, "text_y", 0.5),
            ],
            scale: self.param(engine, "text_size", 0.4).max(0.0),
            tex_aspect: self.tex_size[0] as f32 / self.tex_size[1].max(1) as f32,
            angle: self.param(engine, "text_rot", 0.0).to_radians(),
            _pad: 0.0,
        };
        ctx.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Text Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}

/// Read a font file, or `None` if it is not one we can parse.
fn load_font(path: &Path) -> Option<FontVec> {
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec(bytes).ok()
}

/// Somewhere to start when no font has been picked. The OS font directories are
/// all we look at — a font shipped with the app would be a licence question.
pub fn default_font() -> Option<PathBuf> {
    for dir in font_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_font(p))
            .collect();
        candidates.sort();
        if let Some(first) = candidates.into_iter().find(|p| load_font(p).is_some()) {
            return Some(first);
        }
    }
    None
}

/// Whether a path looks like a font file the rasteriser can open.
pub fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_lowercase().as_str(), "ttf" | "otf" | "ttc"))
}

/// The platform's font directories.
pub fn font_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir();
    #[cfg(target_os = "macos")]
    let dirs = [
        Some(PathBuf::from("/System/Library/Fonts")),
        Some(PathBuf::from("/Library/Fonts")),
        home.map(|h| h.join("Library/Fonts")),
    ];
    #[cfg(target_os = "windows")]
    let dirs = [
        Some(PathBuf::from("C:\\Windows\\Fonts")),
        home.map(|h| h.join("AppData/Local/Microsoft/Windows/Fonts")),
        None,
    ];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let dirs = [
        Some(PathBuf::from("/usr/share/fonts")),
        Some(PathBuf::from("/usr/local/share/fonts")),
        home.map(|h| h.join(".local/share/fonts")),
    ];
    dirs.into_iter().flatten().filter(|d| d.is_dir()).collect()
}

/// One glyph placed in the raster, in pixels from the top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placed {
    id: GlyphId,
    x: f32,
    y: f32,
}

/// Lay the string out at [`RASTER_PX`], returning the glyphs and the raster size
/// they need. Newlines break lines; everything else is one run — no shaping, so
/// this is Latin-shaped text (see the module docs).
fn layout(font: &FontVec, l: &Layout) -> (Vec<Placed>, u32, u32) {
    let scaled = font.as_scaled(PxScale::from(RASTER_PX));
    let step = (scaled.ascent() - scaled.descent() + scaled.line_gap()) * l.line_height;
    let tracking = l.tracking * RASTER_PX;

    // Lay each line out from x=0, keeping its width so alignment can shift it.
    let mut lines: Vec<(Vec<Placed>, f32)> = Vec::new();
    for (row, line) in l.text.split('\n').enumerate() {
        let mut placed = Vec::new();
        let mut x = 0.0_f32;
        let mut prev: Option<GlyphId> = None;
        let y = scaled.ascent() + row as f32 * step;
        for c in line.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            placed.push(Placed { id, x, y });
            x += scaled.h_advance(id) + tracking;
            prev = Some(id);
        }
        // The trailing tracking is not part of the line.
        let width = (x - tracking).max(0.0);
        lines.push((placed, width));
    }

    let text_w = lines.iter().map(|(_, w)| *w).fold(0.0_f32, f32::max);
    let rows = lines.len() as f32;
    let text_h = scaled.ascent() - scaled.descent() + (rows - 1.0) * step;

    let mut glyphs = Vec::new();
    for (placed, width) in lines {
        let shift = match l.align {
            0 => 0.0,
            2 => text_w - width,
            _ => (text_w - width) / 2.0,
        };
        glyphs.extend(placed.into_iter().map(|p| Placed {
            x: p.x + shift + PAD as f32,
            ..p
        }));
    }

    let w = (text_w.ceil() as u32 + PAD * 2).clamp(1, MAX_RASTER_W);
    let h = (text_h.ceil() as u32 + PAD * 2).clamp(1, MAX_RASTER_W);
    (glyphs, w, h)
}

/// Rasterise white glyphs with coverage in alpha, so the blit can tint them.
fn raster(font: &FontVec, l: &Layout) -> (Vec<u8>, u32, u32) {
    let (glyphs, w, h) = layout(font, l);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for g in glyphs {
        let glyph = g
            .id
            .with_scale_and_position(RASTER_PX, ab_glyph::point(g.x, g.y + PAD as f32));
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                return;
            }
            let i = ((py as u32 * w + px as u32) * 4) as usize;
            let a = (coverage * 255.0) as u8;
            // Glyphs can overlap (tight tracking, accents): keep the strongest
            // coverage rather than adding, which would ring at the joins.
            if a > pixels[i + 3] {
                pixels[i] = 255;
                pixels[i + 1] = 255;
                pixels[i + 2] = 255;
                pixels[i + 3] = a;
            }
        });
    }
    (pixels, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first font the platform offers, or the test is meaningless.
    fn a_font() -> Option<FontVec> {
        default_font().as_deref().and_then(load_font)
    }

    /// A shader that will not parse is a panic inside `create_shader_module`
    /// the first time someone adds a text layer — which is exactly how a field
    /// named `target`, a WGSL reserved word, got in.
    #[test]
    fn the_blit_shader_parses() {
        wgpu::naga::front::wgsl::parse_str(include_str!("text.wgsl"))
            .expect("text.wgsl must be valid WGSL");
    }

    #[test]
    fn a_longer_string_needs_a_wider_raster() {
        let Some(font) = a_font() else { return };
        let short = layout(&font, &Layout { text: "A".into(), ..Layout::default() });
        let long = layout(&font, &Layout { text: "AAAA".into(), ..Layout::default() });
        assert!(long.1 > short.1, "{} should exceed {}", long.1, short.1);
        assert_eq!(long.2, short.2, "one line either way");
        assert_eq!(long.0.len(), 4);
    }

    #[test]
    fn every_line_adds_height() {
        let Some(font) = a_font() else { return };
        let one = layout(&font, &Layout { text: "A".into(), ..Layout::default() });
        let two = layout(&font, &Layout { text: "A\nB".into(), ..Layout::default() });
        assert!(two.2 > one.2);
        assert_eq!(two.0.len(), 2);
    }

    /// Centred, a short line sits inside the block; left-aligned it starts at
    /// the same place as the long one.
    #[test]
    fn alignment_shifts_the_short_line() {
        let Some(font) = a_font() else { return };
        let text = "AAAA\nA";
        let centred = layout(&font, &Layout { text: text.into(), align: 1, ..Layout::default() });
        let left = layout(&font, &Layout { text: text.into(), align: 0, ..Layout::default() });
        let last = |g: &Vec<Placed>| g.last().unwrap().x;
        assert!(last(&centred.0) > last(&left.0));
        assert_eq!(left.0.first().unwrap().x, last(&left.0), "left edge shared");
    }

    /// An empty string still produces a texture the pipeline can bind.
    #[test]
    fn empty_text_still_has_a_raster() {
        let Some(font) = a_font() else { return };
        let (pixels, w, h) = raster(&font, &Layout { text: String::new(), ..Layout::default() });
        assert!(w >= 1 && h >= 1);
        assert_eq!(pixels.len(), (w * h * 4) as usize);
    }
}
