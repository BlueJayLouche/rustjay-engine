//! Video matrix stage — composites N grid cells of the input into one output.
//!
//! Each [`GridCellMapping`] samples a `source_rect` of the input and writes it
//! into a `dest_rect` of the output, with a 0/90/180/270 rotation. Unmapped
//! output area is filled with the background colour. This is the "video wall /
//! HDMI matrix" case: one framebuffer carrying several independent screens.
//!
//! The geometry is **output-cell authoritative**: a cell carries its own grid
//! position, aspect and orientation (set by config/UI; auto-detection only
//! suggests them). See the videowall design notes.

use crate::stage::ProjectionStage;
use bytemuck::{Pod, Zeroable};
use rustjay_core::RenderCtx;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Maximum cell mappings the GPU storage array holds (must match `matrix.wgsl`).
pub const MAX_MAPPINGS: usize = 16;

// ---------------------------------------------------------------------------
// Config model (pure data — no GPU, serde-friendly)
// ---------------------------------------------------------------------------

/// A rectangle in normalized UV space (0..1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }
}

impl Default for Rect {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }
    }
}

/// Grid dimensions in columns × rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridSize {
    pub columns: u32,
    pub rows: u32,
}

impl GridSize {
    pub fn new(columns: u32, rows: u32) -> Self {
        Self { columns, rows }
    }
    pub fn total(&self) -> u32 {
        self.columns * self.rows
    }
    /// (col, row) for a row-major display id.
    pub fn position_from_id(&self, id: u32) -> (u32, u32) {
        let cols = self.columns.max(1);
        (id % cols, id / cols)
    }
    pub fn id_from_position(&self, col: u32, row: u32) -> u32 {
        row * self.columns + col
    }
}

impl Default for GridSize {
    fn default() -> Self {
        Self::new(2, 2)
    }
}

/// Aspect ratio of a mapped screen (drives source-rect sizing in detection/UI;
/// the GPU stage itself reads only the resulting source/dest rects).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AspectRatio {
    Ratio4_3,
    #[default]
    Ratio16_9,
    Ratio16_10,
    Ratio1_1,
    Ratio21_9,
    Custom { w: u32, h: u32 },
}

impl AspectRatio {
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::Ratio4_3 => 4.0 / 3.0,
            Self::Ratio16_9 => 16.0 / 9.0,
            Self::Ratio16_10 => 16.0 / 10.0,
            Self::Ratio1_1 => 1.0,
            Self::Ratio21_9 => 21.0 / 9.0,
            Self::Custom { w, h } => *w as f32 / *h.max(&1) as f32,
        }
    }

    pub fn name(&self) -> String {
        match self {
            Self::Ratio4_3 => "4:3".into(),
            Self::Ratio16_9 => "16:9".into(),
            Self::Ratio16_10 => "16:10".into(),
            Self::Ratio1_1 => "1:1".into(),
            Self::Ratio21_9 => "21:9".into(),
            Self::Custom { w, h } => format!("{w}:{h}"),
        }
    }

    /// Snap a measured width/height to the nearest standard ratio (else Custom).
    pub fn detect(width: f32, height: f32) -> Self {
        if width <= 0.0 || height <= 0.0 {
            return Self::Ratio16_9;
        }
        let ratio = width / height;
        let standards = [
            (Self::Ratio4_3, 4.0 / 3.0),
            (Self::Ratio16_9, 16.0 / 9.0),
            (Self::Ratio16_10, 16.0 / 10.0),
            (Self::Ratio1_1, 1.0),
            (Self::Ratio21_9, 21.0 / 9.0),
        ];
        let (mut best, mut min_diff) = (Self::Ratio16_9, f32::MAX);
        for (ar, val) in standards {
            let d = (ratio - val).abs();
            if d < min_diff {
                min_diff = d;
                best = ar;
            }
        }
        if min_diff < 0.1 {
            best
        } else {
            Self::Custom { w: (width * 100.0) as u32, h: (height * 100.0) as u32 }
        }
    }
}

/// Screen rotation. The shader applies this once; the CPU side never re-rotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Orientation {
    #[default]
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
}

impl Orientation {
    pub fn degrees(&self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::Rotated90 => 90,
            Self::Rotated180 => 180,
            Self::Rotated270 => 270,
        }
    }
    fn gpu_index(&self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Rotated90 => 1,
            Self::Rotated180 => 2,
            Self::Rotated270 => 3,
        }
    }
}

/// Position of a cell in the output grid (units = grid cells, may be fractional).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GridPosition {
    pub col: f32,
    pub row: f32,
    pub width: f32,
    pub height: f32,
}

impl GridPosition {
    pub fn new(col: f32, row: f32, width: f32, height: f32) -> Self {
        Self { col, row, width, height }
    }
    /// Convert to a normalized 0..1 rect given the output grid dimensions.
    pub fn to_normalized_rect(&self, total_cols: u32, total_rows: u32) -> Rect {
        let c = total_cols.max(1) as f32;
        let r = total_rows.max(1) as f32;
        Rect::new(self.col / c, self.row / r, self.width / c, self.height / r)
    }
}

impl Default for GridPosition {
    fn default() -> Self {
        Self::new(0.0, 0.0, 1.0, 1.0)
    }
}

/// Mapping from an input region to an output cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridCellMapping {
    /// Input grid cell index (row-major), used when `custom_source_rect` is None.
    pub input_cell: usize,
    pub output_position: GridPosition,
    pub aspect_ratio: AspectRatio,
    pub orientation: Orientation,
    pub enabled: bool,
    /// Display id for reference (e.g. AprilTag id).
    pub display_id: Option<u32>,
    /// Cell centre in **aspect-neutral wall units** (detection pixels / image
    /// HEIGHT, for BOTH axes — pixels are square, so the photo aspect only widens
    /// the x-range). Resolved into a source rect per-target via [`resolve_source_rects`].
    #[serde(default)]
    pub wall_center: Option<[f32; 2]>,
    /// Cell size in aspect-neutral wall units; `wall_size.x / wall_size.y` is the
    /// display aspect (portrait when the screen is rotated).
    #[serde(default)]
    pub wall_size: Option<[f32; 2]>,
    /// Optional manual override: a source rect in target UV, used verbatim
    /// (after the wall model) when present. Edited by the UI's per-cell nudge.
    pub custom_source_rect: Option<Rect>,
    /// Per-display output adjustments (applied after sampling, in the shader)
    /// to match brightness across physical screens. 1.0 = no change.
    #[serde(default = "one_f32")]
    pub brightness: f32,
    #[serde(default = "one_f32")]
    pub contrast: f32,
    #[serde(default = "one_f32")]
    pub gamma: f32,
}

fn one_f32() -> f32 {
    1.0
}

impl GridCellMapping {
    pub fn new(input_cell: usize, output_position: GridPosition) -> Self {
        Self {
            input_cell,
            output_position,
            aspect_ratio: AspectRatio::default(),
            orientation: Orientation::default(),
            enabled: true,
            display_id: None,
            wall_center: None,
            wall_size: None,
            custom_source_rect: None,
            brightness: 1.0,
            contrast: 1.0,
            gamma: 1.0,
        }
    }

    pub fn with_aspect_ratio(mut self, ratio: AspectRatio) -> Self {
        self.aspect_ratio = ratio;
        self
    }
    pub fn with_orientation(mut self, o: Orientation) -> Self {
        self.orientation = o;
        self
    }
    pub fn with_display_id(mut self, id: u32) -> Self {
        self.display_id = Some(id);
        self
    }
    pub fn with_source_rect(mut self, rect: Rect) -> Self {
        self.custom_source_rect = Some(rect);
        self
    }
    /// Set the aspect-neutral wall geometry (centre + size in wall units).
    pub fn with_wall_geometry(mut self, center: [f32; 2], size: [f32; 2]) -> Self {
        self.wall_center = Some(center);
        self.wall_size = Some(size);
        self
    }

    /// Even input-grid cell — the fallback used when a cell carries no wall
    /// geometry and no manual override (degenerate layout).
    fn grid_cell_rect(&self, input_grid: GridSize) -> Rect {
        let cols = input_grid.columns.max(1) as f32;
        let rows = input_grid.rows.max(1) as f32;
        let col = (self.input_cell % input_grid.columns.max(1) as usize) as f32;
        let row = (self.input_cell / input_grid.columns.max(1) as usize) as f32;
        Rect::new(col / cols, row / rows, 1.0 / cols, 1.0 / rows)
    }

    /// Source rect for UI display / nudging. Resolves the wall geometry against a
    /// representative target aspect (the cell's own grid) when no override exists.
    /// Prefer [`resolve_source_rects`] for rendering (it shares one bbox fit).
    pub fn source_rect(&self, input_grid: GridSize) -> Rect {
        if let Some(r) = self.custom_source_rect {
            return r;
        }
        self.grid_cell_rect(input_grid)
    }

    /// Destination rect in output UV.
    pub fn dest_rect(&self, output_grid: GridSize) -> Rect {
        self.output_position.to_normalized_rect(output_grid.columns, output_grid.rows)
    }
}

/// Resolve the source rect (in target UV) for every **enabled** cell of `config`,
/// for a target of aspect `A = target_w / target_h`, via a **single uniform fit**
/// of the wall layout's bounding box into the target. Positions AND sizes share
/// one scalar scale, so the layout's aspects and relative spacing are preserved on
/// any input ("resized to optimise coverage, but never reformed").
///
/// Returned in the same order as [`VideoMatrixConfig::enabled_mappings`]. A cell's
/// `custom_source_rect`, when present, is used verbatim (manual override, after the
/// wall model). Cells without wall geometry fall back to their even grid cell.
pub fn resolve_source_rects(config: &VideoMatrixConfig, target_aspect: f32) -> Vec<Rect> {
    let a = target_aspect.max(1e-4);
    let input_grid = config.input_grid.grid_size;

    // Wall bbox over enabled cells that carry wall geometry (in wall units).
    let mut minx = f32::MAX;
    let mut miny = f32::MAX;
    let mut maxx = f32::MIN;
    let mut maxy = f32::MIN;
    let mut have_wall = false;
    for m in config.enabled_mappings() {
        if let (Some(c), Some(s)) = (m.wall_center, m.wall_size) {
            have_wall = true;
            minx = minx.min(c[0] - s[0] / 2.0);
            miny = miny.min(c[1] - s[1] / 2.0);
            maxx = maxx.max(c[0] + s[0] / 2.0);
            maxy = maxy.max(c[1] + s[1] / 2.0);
        }
    }
    let bbox_w = maxx - minx;
    let bbox_h = maxy - miny;
    let degenerate = !have_wall || bbox_w <= 0.0 || bbox_h <= 0.0;

    // Uniform scale (target-height-units per wall-unit), maximising coverage; one
    // scalar shared by positions AND sizes — never per-axis.
    let scale = (a / bbox_w).min(1.0 / bbox_h);
    let ox = (a - bbox_w * scale) / 2.0;
    let oy = (1.0 - bbox_h * scale) / 2.0;

    config
        .enabled_mappings()
        .map(|m| {
            if let Some(r) = m.custom_source_rect {
                return r;
            }
            match (m.wall_center, m.wall_size) {
                (Some(c), Some(s)) if !degenerate => {
                    let x_thu = (c[0] - s[0] / 2.0 - minx) * scale + ox;
                    let y_thu = (c[1] - s[1] / 2.0 - miny) * scale + oy;
                    let w_thu = s[0] * scale;
                    let h_thu = s[1] * scale;
                    Rect::new(x_thu / a, y_thu, w_thu / a, h_thu)
                }
                _ => m.grid_cell_rect(input_grid),
            }
        })
        .collect()
}

/// Input texture grid subdivision + the cell mappings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputGridConfig {
    pub grid_size: GridSize,
    pub mappings: Vec<GridCellMapping>,
}

impl InputGridConfig {
    pub fn new(grid_size: GridSize) -> Self {
        Self { grid_size, mappings: Vec::new() }
    }
    pub fn add_mapping(&mut self, m: GridCellMapping) {
        self.mappings.push(m);
    }
    /// Map each cell to the matching output position (cell 0 → (0,0), …).
    pub fn create_default_mapping(&mut self) {
        self.mappings.clear();
        let cols = self.grid_size.columns.max(1) as usize;
        for i in 0..self.grid_size.total() as usize {
            let (col, row) = ((i % cols) as f32, (i / cols) as f32);
            self.mappings.push(GridCellMapping::new(i, GridPosition::new(col, row, 1.0, 1.0)));
        }
    }
}

impl Default for InputGridConfig {
    fn default() -> Self {
        Self::new(GridSize::new(3, 3))
    }
}

/// Complete video-matrix configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoMatrixConfig {
    pub input_grid: InputGridConfig,
    pub output_grid: GridSize,
    /// RGBA background for unmapped output area.
    pub background_color: [f32; 4],
    /// Aspect (W/H) of the matrix output framebuffer (the signal the HDMI matrix
    /// receives). The mapping is laid out in this fixed aspect and letterboxed into
    /// the actual window, so resizing the window scales uniformly instead of
    /// stretching the mapping. Defaults to 16:9.
    #[serde(default = "default_output_aspect")]
    pub output_aspect: f32,
}

fn default_output_aspect() -> f32 {
    16.0 / 9.0
}

impl VideoMatrixConfig {
    /// Output grid defaults to match the input grid.
    pub fn new(input_grid_size: GridSize) -> Self {
        Self {
            input_grid: InputGridConfig::new(input_grid_size),
            output_grid: input_grid_size,
            background_color: [0.0, 0.0, 0.0, 1.0],
            output_aspect: default_output_aspect(),
        }
    }
    pub fn enabled_mappings(&self) -> impl Iterator<Item = &GridCellMapping> {
        self.input_grid.mappings.iter().filter(|m| m.enabled)
    }
}

impl Default for VideoMatrixConfig {
    fn default() -> Self {
        Self::new(GridSize::new(3, 3))
    }
}

// ---------------------------------------------------------------------------
// UI ↔ stage handoff (matches the RotationSync idiom)
// ---------------------------------------------------------------------------

/// Shared config between the UI (writer) and the stage (reader). Bump `version`
/// on any change so the stage re-uploads its GPU buffers.
#[derive(Debug, Clone, Default)]
pub struct MatrixSync {
    pub config: VideoMatrixConfig,
    pub version: u64,
}

impl MatrixSync {
    pub fn set_config(&mut self, config: VideoMatrixConfig) {
        self.config = config;
        self.version = self.version.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------------
// GPU structs (must match matrix.wgsl exactly)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CellMappingGpu {
    source_rect: [f32; 4],
    dest_rect: [f32; 4],
    orientation: u32,
    enabled: u32,
    brightness: f32,
    contrast: f32,
    gamma: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

impl CellMappingGpu {
    /// `source` is the already-resolved source rect (target UV), shared via the
    /// uniform wall-bbox fit — see [`resolve_source_rects`].
    fn from_mapping(m: &GridCellMapping, source: Rect, output: GridSize) -> Self {
        let s = source;
        let d = m.dest_rect(output);
        Self {
            source_rect: [s.x, s.y, s.width, s.height],
            dest_rect: [d.x, d.y, d.width, d.height],
            orientation: m.orientation.gpu_index(),
            enabled: m.enabled as u32,
            brightness: m.brightness,
            contrast: m.contrast,
            gamma: m.gamma.max(0.01),
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
    fn disabled() -> Self {
        Self {
            source_rect: [0.0; 4],
            dest_rect: [0.0; 4],
            orientation: 0,
            enabled: 0,
            brightness: 1.0,
            contrast: 1.0,
            gamma: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct MatrixUniformsGpu {
    mapping_count: u32,
    output_width: u32,
    output_height: u32,
    output_aspect: f32,
    background_color: [f32; 4],
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// Projection stage that composites a [`VideoMatrixConfig`] into the output.
pub struct MatrixStage {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    uniforms_buffer: wgpu::Buffer,
    mappings_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    sync: Arc<Mutex<MatrixSync>>,
    last_version: u64,
    mapping_count: u32,
    background: [f32; 4],
    output_size: [u32; 2],
    /// Fixed aspect the mapping is laid out in (letterboxed into the window).
    output_aspect: f32,
    /// Aspect (W/H) of the **input** content the source rects were last resolved
    /// for; the wall layout is uniformly fitted into this so it never distorts.
    target_aspect: f32,

    cached_tex_bind_group: Option<wgpu::BindGroup>,
    cached_input_ptr: Option<usize>,
}

impl MatrixStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: Arc<Mutex<MatrixSync>>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Matrix Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/matrix.wgsl").into()),
        });

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Matrix Texture BGL"),
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

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Matrix Uniform BGL"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Matrix Pipeline Layout"),
            bind_group_layouts: &[Some(&tex_bgl), Some(&uniform_bgl)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Matrix Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // ponytail: cache one sampler — the mapper rebuilt it every frame.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Matrix Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Matrix Uniforms"),
            size: std::mem::size_of::<MatrixUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mappings_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Matrix Mappings"),
            size: (std::mem::size_of::<CellMappingGpu>() * MAX_MAPPINGS) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Buffers never reallocate, so the uniform bind group is built once.
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Matrix Uniform Bind Group"),
            layout: &uniform_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: mappings_buffer.as_entire_binding() },
            ],
        });

        Self {
            pipeline,
            sampler,
            uniforms_buffer,
            mappings_buffer,
            uniform_bind_group,
            sync,
            last_version: u64::MAX, // force first upload
            mapping_count: 0,
            background: [0.0, 0.0, 0.0, 1.0],
            output_size: [0, 0],
            output_aspect: 16.0 / 9.0,
            target_aspect: 16.0 / 9.0,
            cached_tex_bind_group: None,
            cached_input_ptr: None,
        }
    }

    /// Re-pack and upload the cell mappings + uniforms from the current config.
    /// Source rects are resolved against `self.target_aspect` (the input content's
    /// aspect) via the uniform wall-bbox fit, so the layout never distorts.
    fn upload(&mut self, queue: &wgpu::Queue, config: &VideoMatrixConfig) {
        let output = config.output_grid;

        let source_rects = resolve_source_rects(config, self.target_aspect);
        let mut cells: Vec<CellMappingGpu> = config
            .enabled_mappings()
            .zip(source_rects)
            .take(MAX_MAPPINGS)
            .map(|(m, src)| CellMappingGpu::from_mapping(m, src, output))
            .collect();
        self.mapping_count = cells.len() as u32;
        cells.resize(MAX_MAPPINGS, CellMappingGpu::disabled());
        queue.write_buffer(&self.mappings_buffer, 0, bytemuck::cast_slice(&cells));

        self.background = config.background_color;
        self.output_aspect = config.output_aspect.max(0.01);
        self.write_uniforms(queue);
    }

    fn write_uniforms(&self, queue: &wgpu::Queue) {
        let u = MatrixUniformsGpu {
            mapping_count: self.mapping_count,
            output_width: self.output_size[0].max(1),
            output_height: self.output_size[1].max(1),
            output_aspect: self.output_aspect,
            background_color: self.background,
        };
        queue.write_buffer(&self.uniforms_buffer, 0, bytemuck::bytes_of(&u));
    }
}

impl ProjectionStage for MatrixStage {
    fn label(&self) -> &str {
        "matrix"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    ) {
        // Re-upload config when the UI bumps the version.
        let (config, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.config.clone(), g.version)
        };
        // Derive the target aspect from the input CONTENT (not the window): the
        // wall layout is uniformly fitted into it so it never distorts when the
        // input aspect differs from the calibration photo. Falls back to keeping
        // the last value when the texture is unavailable.
        if let Some(tex) = input_texture {
            let a = tex.width() as f32 / tex.height().max(1) as f32;
            if (a - self.target_aspect).abs() > 1e-4 {
                self.target_aspect = a;
                self.last_version = u64::MAX; // force re-resolve below
            }
        }
        let mut needs_uniform_write = false;
        if self.output_size != output_size {
            self.output_size = output_size;
            needs_uniform_write = true;
        }
        if self.last_version != version {
            self.last_version = version;
            self.upload(ctx.queue, &config); // writes uniforms too
        } else if needs_uniform_write {
            self.write_uniforms(ctx.queue);
        }

        let input_ptr = input as *const _ as usize;
        if self.cached_input_ptr != Some(input_ptr) || self.cached_tex_bind_group.is_none() {
            self.cached_tex_bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Matrix Texture Bind Group"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
            self.cached_input_ptr = Some(input_ptr);
        }
        let tex_bind_group = self.cached_tex_bind_group.as_ref().unwrap();

        let bg = self.background;
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Matrix Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: bg[0] as f64,
                        g: bg[1] as f64,
                        b: bg[2] as f64,
                        a: bg[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, tex_bind_group, &[]);
        pass.set_bind_group(1, &self.uniform_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_tex_bind_group = None;
        self.cached_input_ptr = None;
    }

    fn is_active(&self) -> bool {
        let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        g.config.input_grid.mappings.iter().any(|m| m.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let mut cfg = VideoMatrixConfig::new(GridSize::new(2, 2));
        cfg.input_grid.create_default_mapping();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VideoMatrixConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(back.input_grid.mappings.len(), 4);
    }

    #[test]
    fn source_and_dest_rects() {
        let m = GridCellMapping::new(0, GridPosition::new(1.0, 0.0, 1.0, 1.0));
        // No intrinsic geometry → falls back to the even grid cell.
        let s = m.source_rect(GridSize::new(3, 3));
        assert!((s.width - 1.0 / 3.0).abs() < 1e-5);
        let d = m.dest_rect(GridSize::new(2, 1));
        assert!((d.x - 0.5).abs() < 1e-5 && (d.width - 0.5).abs() < 1e-5);
    }

    // A stored bezel rect is returned directly (anchored to the wall), independent
    // of any input/grid.
    #[test]
    fn source_rect_uses_stored_bezel() {
        let r = Rect::new(0.2, 0.1, 0.3, 0.4);
        let m = GridCellMapping::new(0, GridPosition::default()).with_source_rect(r);
        assert_eq!(m.source_rect(GridSize::new(3, 3)), r);
    }

    /// Build a config from aspect-neutral wall cells (centre, size).
    fn wall_config(cells: &[([f32; 2], [f32; 2])]) -> VideoMatrixConfig {
        let mut cfg = VideoMatrixConfig::new(GridSize::new(cells.len().max(1) as u32, 1));
        for (i, (c, s)) in cells.iter().enumerate() {
            cfg.input_grid
                .add_mapping(GridCellMapping::new(i, GridPosition::default()).with_wall_geometry(*c, *s));
        }
        cfg
    }

    // The core invariant: resolve the SAME wall layout for several target aspects.
    // For every aspect, each cell's pixel-aspect must equal its display aspect,
    // and the centres must scale uniformly (no per-axis spread between cells).
    #[test]
    fn layout_invariant_across_target_aspects() {
        // Two 16:9 cells (1.778) and one 4:3 cell (1.333), spread along x.
        let cells = [
            ([0.5, 0.5], [16.0 / 9.0, 1.0]),
            ([2.5, 0.5], [16.0 / 9.0, 1.0]),
            ([2.0, 1.6], [4.0 / 3.0, 1.0]),
        ];
        let cfg = wall_config(&cells);
        let display_aspects = [16.0 / 9.0_f32, 16.0 / 9.0, 4.0 / 3.0];

        let aspects = [16.0 / 9.0_f32, 4.0 / 3.0, 9.0 / 16.0];
        // Reference vector between cell0 and cell1 centres, in wall units.
        let ref_dx = cells[1].0[0] - cells[0].0[0];
        let ref_dy = cells[1].0[1] - cells[0].0[1];

        let mut prev_scale_x: Option<f32> = None;
        for a in aspects {
            let rects = resolve_source_rects(&cfg, a);
            assert_eq!(rects.len(), 3);

            // Per cell: pixel-aspect (UV·target px) == display aspect.
            for (r, da) in rects.iter().zip(display_aspects) {
                let pixel_aspect = (r.width * a) / r.height;
                assert!(
                    (pixel_aspect - da).abs() < 1e-3,
                    "A={a}: pixel aspect {pixel_aspect} != display {da}",
                );
            }

            // Centres scale uniformly: every cell→cell displacement, mapped back to
            // wall units (centre.x is UV → ×a to undo the /a), must equal the wall
            // reference times ONE shared scalar in BOTH axes (no per-axis spread).
            let centre = |r: &Rect| [r.x * a + r.width * a / 2.0, r.y + r.height / 2.0];
            let c0 = centre(&rects[0]);
            let c1 = centre(&rects[1]);
            let c2 = centre(&rects[2]);
            let sx = (c1[0] - c0[0]) / ref_dx; // shared scalar from a pure-x pair
            // cell0→cell2 has BOTH dx and dy: the same scalar must reproduce both.
            let ref_dx2 = cells[2].0[0] - cells[0].0[0];
            let ref_dy2 = cells[2].0[1] - cells[0].0[1];
            assert!((c2[0] - c0[0] - ref_dx2 * sx).abs() < 1e-3, "A={a}: x not uniform");
            assert!((c2[1] - c0[1] - ref_dy2 * sx).abs() < 1e-3, "A={a}: y not uniform (per-axis spread)");
            assert!((c1[1] - c0[1] - ref_dy * sx).abs() < 1e-3, "A={a}: y spread detected");
            prev_scale_x = Some(sx);
        }
        assert!(prev_scale_x.is_some());
    }

    // A rotated 4:3 cell is stored portrait (wall_size.x < wall_size.y) and stays
    // portrait after resolution for any target.
    #[test]
    fn rotated_cell_is_portrait() {
        let cfg = wall_config(&[([0.5, 0.5], [3.0 / 4.0, 1.0])]); // portrait 3:4
        let m = &cfg.input_grid.mappings[0];
        let s = m.wall_size.unwrap();
        assert!(s[0] < s[1], "expected portrait wall_size, got {s:?}");
        for a in [16.0 / 9.0_f32, 4.0 / 3.0, 9.0 / 16.0] {
            let r = resolve_source_rects(&cfg, a)[0];
            let pixel_aspect = (r.width * a) / r.height;
            assert!((pixel_aspect - 3.0 / 4.0).abs() < 1e-3, "A={a}: aspect {pixel_aspect}");
        }
    }

    // No wall geometry → fall back to the even grid cell (degenerate layout).
    #[test]
    fn resolve_falls_back_to_grid_without_wall() {
        let mut cfg = VideoMatrixConfig::new(GridSize::new(2, 1));
        cfg.input_grid.create_default_mapping();
        let rects = resolve_source_rects(&cfg, 16.0 / 9.0);
        assert_eq!(rects.len(), 2);
        assert!((rects[0].width - 0.5).abs() < 1e-5);
        assert!((rects[1].x - 0.5).abs() < 1e-5);
    }

    // A custom_source_rect override is returned verbatim, after the wall model.
    #[test]
    fn resolve_uses_custom_override() {
        let mut cfg = wall_config(&[([0.5, 0.5], [16.0 / 9.0, 1.0])]);
        let r = Rect::new(0.1, 0.2, 0.3, 0.4);
        cfg.input_grid.mappings[0].custom_source_rect = Some(r);
        assert_eq!(resolve_source_rects(&cfg, 16.0 / 9.0)[0], r);
    }

    #[test]
    fn gpu_struct_sizes() {
        // vec4-aligned WGSL structs: uniforms = 32 bytes, cell stride = 64 bytes.
        assert_eq!(std::mem::size_of::<MatrixUniformsGpu>(), 32);
        assert_eq!(std::mem::size_of::<CellMappingGpu>(), 64);
    }

    #[test]
    fn aspect_detect() {
        assert_eq!(AspectRatio::detect(1920.0, 1080.0), AspectRatio::Ratio16_9);
        assert_eq!(AspectRatio::detect(1024.0, 768.0), AspectRatio::Ratio4_3);
    }

    // GPU golden test: map the input into the LEFT half of a 2×2 output; the
    // right half must stay background. Solid-white input keeps it filter-proof.
    #[test]
    fn matrix_left_half_snapshot() {
        let (device, queue) = pollster::block_on(init_wgpu());
        let (_in_tex, in_view) = solid_texture(&device, &queue, [255, 255, 255, 255]);
        let (out_tex, out_view) = output_texture(&device, 2, 2);

        let mut cfg = VideoMatrixConfig::new(GridSize::new(1, 1));
        cfg.input_grid.add_mapping(
            GridCellMapping::new(0, GridPosition::new(0.0, 0.0, 0.5, 1.0)), // left half of 1×1
        );
        let sync = Arc::new(Mutex::new(MatrixSync { config: cfg, version: 1 }));

        let mut stage = MatrixStage::new(&device, wgpu::TextureFormat::Rgba8Unorm, sync);
        let mut encoder = device.create_command_encoder(&Default::default());
        let dummy_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let mut ctx = RenderCtx {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            vertex_buffer: &dummy_vb,
        };
        stage.render(&mut ctx, &in_view, Some(&_in_tex), &out_view, [2, 2]);
        queue.submit(std::iter::once(encoder.finish()));

        let px = readback_rgba8(&device, &queue, &out_tex, 2, 2);
        // Row-major 2×2: col0 = white (mapped), col1 = black (background).
        assert_eq!(&px[0..4], &[255, 255, 255, 255]); // TL
        assert_eq!(&px[4..8], &[0, 0, 0, 255]); // TR
        assert_eq!(&px[8..12], &[255, 255, 255, 255]); // BL
        assert_eq!(&px[12..16], &[0, 0, 0, 255]); // BR
    }

    // --- test helpers (mirror identity.rs) ---

    async fn init_wgpu() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .expect("no adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                label: Some("Test Device"),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .expect("no device")
    }

    fn solid_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: [u8; 4],
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let size = wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Test Input"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let data: Vec<u8> = rgba.iter().cycle().take(2 * 2 * 4).copied().collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(2 * 4), rows_per_image: None },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn output_texture(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Test Output"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn readback_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = buffer.slice(..);
        let mapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mc = Arc::clone(&mapped);
        slice.map_async(wgpu::MapMode::Read, move |_| {
            mc.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        while !mapped.load(std::sync::atomic::Ordering::SeqCst)
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            device.poll(wgpu::PollType::Poll).ok();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let s = (row * bytes_per_row) as usize;
            out.extend_from_slice(&data[s..s + (width * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }
}
