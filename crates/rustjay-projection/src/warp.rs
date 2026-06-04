//! Warp pipeline — perspective correction and UV mesh warping for projection mapping.

use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

// ── Mesh types ───────────────────────────────────────────────────────────

/// A single point in a UV warp mesh: output-space position + source-space UV.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct MeshPoint {
    /// Position in output-normalized coords [0..1]
    pub position: [f32; 2],
    /// UV coordinates in source texture space [0..1]
    pub uv: [f32; 2],
}

/// A grid of XYUV warp points defining an arbitrary mesh warp.
///
/// Points are stored row-major: `points[row * cols + col]`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WarpMesh {
    /// Number of columns in the grid (≥2)
    pub cols: u32,
    /// Number of rows in the grid (≥2)
    pub rows: u32,
    /// Grid points, row-major order. Length = cols × rows.
    pub points: Vec<MeshPoint>,
}

impl WarpMesh {
    /// Create an identity mesh (no warp) with the given grid dimensions.
    pub fn identity(cols: u32, rows: u32) -> Self {
        let mut points = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            let v = r as f32 / (rows - 1).max(1) as f32;
            for c in 0..cols {
                let u = c as f32 / (cols - 1).max(1) as f32;
                points.push(MeshPoint { position: [u, v], uv: [u, v] });
            }
        }
        Self { cols, rows, points }
    }

    /// Create a mesh from 4 corner positions (corner-pin equivalent).
    /// Order: TL, TR, BR, BL → grid row-major: TL, TR, BL, BR
    pub fn from_corners(corners: &[[f32; 2]; 4]) -> Self {
        Self {
            cols: 2,
            rows: 2,
            points: vec![
                MeshPoint { position: corners[0], uv: [0.0, 0.0] }, // TL
                MeshPoint { position: corners[1], uv: [1.0, 0.0] }, // TR
                MeshPoint { position: corners[3], uv: [0.0, 1.0] }, // BL
                MeshPoint { position: corners[2], uv: [1.0, 1.0] }, // BR
            ],
        }
    }

    /// Check if this mesh is an identity warp (positions == UVs).
    pub fn is_identity(&self) -> bool {
        self.points.iter().all(|p| {
            (p.position[0] - p.uv[0]).abs() < 1e-6 && (p.position[1] - p.uv[1]).abs() < 1e-6
        })
    }
}

/// Warp mode for surface assignments: corner-pin or arbitrary mesh.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum WarpMode {
    /// 4-point corner-pin warp (TL, TR, BR, BL in output space [0..1]).
    CornerPin {
        /// The four corner positions in normalized output coordinates.
        corners: [[f32; 2]; 4]
    },
    /// Arbitrary XYUV mesh warp grid.
    Mesh(WarpMesh),
}

impl WarpMode {
    /// Create a corner-pin warp from 4 corners.
    pub fn corner_pin(corners: [[f32; 2]; 4]) -> Self {
        Self::CornerPin { corners }
    }

    /// Create an identity corner-pin (no warp, unit square).
    pub fn identity() -> Self {
        Self::CornerPin {
            corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        }
    }

    /// Check if this warp mode is an identity (no warp effect).
    pub fn is_identity(&self) -> bool {
        match self {
            Self::CornerPin { corners } => {
                let id = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                corners.iter().zip(id.iter())
                    .all(|(a, b)| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6)
            }
            Self::Mesh(mesh) => mesh.is_identity(),
        }
    }
}

// ── Homography ───────────────────────────────────────────────────────────

/// Compute a forward homography that maps from `src_corners` to `dst_corners`.
/// Returns 12 floats: 3 rows × 4 (xyz + padding), suitable for GPU uniform.
pub fn compute_forward_homography(
    src_corners: &[[f32; 2]; 4],
    dst_corners: &[[f32; 2]; 4],
) -> [f32; 12] {
    let h = solve_homography(src_corners, dst_corners);
    [
        h[0], h[1], h[2], 0.0,
        h[3], h[4], h[5], 0.0,
        h[6], h[7], h[8], 0.0,
    ]
}

fn solve_homography(src: &[[f32; 2]; 4], dst: &[[f32; 2]; 4]) -> [f32; 9] {
    let mut a = [[0.0_f64; 8]; 8];
    let mut b = [0.0_f64; 8];

    for i in 0..4 {
        let (sx, sy) = (src[i][0] as f64, src[i][1] as f64);
        let (dx, dy) = (dst[i][0] as f64, dst[i][1] as f64);
        let row1 = i * 2;
        let row2 = i * 2 + 1;
        a[row1] = [sx, sy, 1.0, 0.0, 0.0, 0.0, -sx * dx, -sy * dx];
        b[row1] = dx;
        a[row2] = [0.0, 0.0, 0.0, sx, sy, 1.0, -sx * dy, -sy * dy];
        b[row2] = dy;
    }

    let h = gauss_solve_8x8(&mut a, &mut b);
    [
        h[0] as f32, h[1] as f32, h[2] as f32,
        h[3] as f32, h[4] as f32, h[5] as f32,
        h[6] as f32, h[7] as f32, 1.0,
    ]
}

#[allow(clippy::needless_range_loop)]
fn gauss_solve_8x8(a: &mut [[f64; 8]; 8], b: &mut [f64; 8]) -> [f64; 8] {
    let n = 8;
    for col in 0..n {
        let mut max_row = col;
        let mut max_val = a[col][col].abs();
        for row in (col + 1)..n {
            if a[row][col].abs() > max_val {
                max_val = a[row][col].abs();
                max_row = row;
            }
        }
        a.swap(col, max_row);
        b.swap(col, max_row);

        let pivot = a[col][col];
        if pivot.abs() < 1e-12 {
            log::warn!("Degenerate homography: pivot near zero");
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        }

        for row in (col + 1)..n {
            let factor = a[row][col] / pivot;
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut x = [0.0_f64; 8];
    for col in (0..n).rev() {
        x[col] = b[col];
        for k in (col + 1)..n {
            x[col] -= a[col][k] * x[k];
        }
        x[col] /= a[col][col];
    }
    x
}

// ── Mesh import/export ───────────────────────────────────────────────────

/// Supported mesh file formats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshFormat {
    /// Paul Bourke XYUV CSV.
    XyuvCsv,
    /// JSON serialization of WarpMesh.
    Json,
}

impl MeshFormat {
    /// Auto-detect format from file extension.
    pub fn from_extension(path: &std::path::Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("csv" | "xyuv" | "txt") => Some(Self::XyuvCsv),
            Some("json") => Some(Self::Json),
            _ => None,
        }
    }
}

impl WarpMesh {
    /// Parse a Paul Bourke XYUV CSV string.
    pub fn from_xyuv_csv(input: &str) -> anyhow::Result<Self> {
        let mut lines = input
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'));

        let header = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("XYUV CSV: missing header"))?;
        let dims: Vec<u32> = header
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if dims.len() < 2 {
            anyhow::bail!("XYUV CSV: header must contain mesh_w mesh_h");
        }
        let cols = dims[0];
        let rows = dims[1];
        if cols < 2 || rows < 2 {
            anyhow::bail!("XYUV CSV: dimensions must be ≥ 2");
        }
        if cols > 10_000 || rows > 10_000 {
            anyhow::bail!("XYUV CSV: dimensions too large");
        }

        let expected = (cols * rows) as usize;
        let mut points = Vec::with_capacity(expected);

        for line in lines {
            let vals: Vec<f32> = line
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            if vals.len() < 4 {
                continue;
            }
            points.push(MeshPoint {
                position: [vals[0], vals[1]],
                uv: [vals[2], vals[3]],
            });
        }

        if points.len() != expected {
            anyhow::bail!(
                "XYUV CSV: expected {} points ({}×{}), got {}",
                expected,
                cols,
                rows,
                points.len()
            );
        }

        Ok(Self { cols, rows, points })
    }

    /// Export to Paul Bourke XYUV CSV format.
    pub fn to_xyuv_csv(&self) -> String {
        let mut out = String::with_capacity(self.points.len() * 40);
        out.push_str(&format!("{} {}\n", self.cols, self.rows));
        for pt in &self.points {
            out.push_str(&format!(
                "{:.6} {:.6} {:.6} {:.6} 1.000000\n",
                pt.position[0], pt.position[1], pt.uv[0], pt.uv[1],
            ));
        }
        out
    }

    /// Load from file with auto-detected format.
    pub fn load_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let format = MeshFormat::from_extension(path)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh file extension: {:?}", path))?;
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read mesh file {:?}: {}", path, e))?;
        match format {
            MeshFormat::XyuvCsv => Self::from_xyuv_csv(&content),
            MeshFormat::Json => {
                let mesh: Self = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("JSON mesh parse error: {}", e))?;
                if mesh.cols < 2 || mesh.rows < 2 {
                    anyhow::bail!("JSON mesh: dimensions must be ≥ 2");
                }
                if mesh.points.len() != (mesh.cols * mesh.rows) as usize {
                    anyhow::bail!("JSON mesh: point count mismatch");
                }
                Ok(mesh)
            }
        }
    }

    /// Save to file with auto-detected format.
    pub fn save_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let format = MeshFormat::from_extension(path)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh file extension: {:?}", path))?;
        let content = match format {
            MeshFormat::XyuvCsv => self.to_xyuv_csv(),
            MeshFormat::Json => serde_json::to_string_pretty(self)
                .map_err(|e| anyhow::anyhow!("JSON mesh serialize error: {}", e))?,
        };
        std::fs::write(path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write mesh file {:?}: {}", path, e))?;
        Ok(())
    }
}

// ── WarpStage ────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WarpParamsUniform {
    h_row0: [f32; 4],
    h_row1: [f32; 4],
    h_row2: [f32; 4],
    use_homography: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// GPU vertex for warp mesh.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WarpVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

impl WarpVertex {
    const fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

/// Warp projection stage — mesh or corner-pin UV warping.
pub struct WarpStage {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    num_indices: u32,
    #[allow(dead_code)]
    is_corner_pin: bool,
}

impl WarpStage {
    /// Create a warp stage from a `WarpMesh`.
    pub fn from_mesh(device: &wgpu::Device, format: wgpu::TextureFormat, mesh: &WarpMesh) -> Self {
        let (vertices, indices) = build_mesh_buffers(mesh);
        let is_corner_pin = mesh.cols == 2 && mesh.rows == 2;
        Self::new(device, format, &vertices, &indices, is_corner_pin)
    }

    /// Create a warp stage from a `WarpMode`.
    pub fn from_mode(device: &wgpu::Device, format: wgpu::TextureFormat, mode: &WarpMode) -> Self {
        match mode {
            WarpMode::CornerPin { corners } => {
                let mesh = WarpMesh::from_corners(corners);
                let (vertices, indices) = build_mesh_buffers(&mesh);
                Self::new(device, format, &vertices, &indices, true)
            }
            WarpMode::Mesh(mesh) => {
                let (vertices, indices) = build_mesh_buffers(mesh);
                Self::new(device, format, &vertices, &indices, false)
            }
        }
    }

    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        vertices: &[WarpVertex],
        indices: &[u16],
        is_corner_pin: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Warp Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/warp.wgsl").into()),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Warp BGL"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Warp Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Warp Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[WarpVertex::desc()],
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Warp Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Params"),
            contents: bytemuck::bytes_of(&WarpParamsUniform {
                h_row0: [1.0, 0.0, 0.0, 0.0],
                h_row1: [0.0, 1.0, 0.0, 0.0],
                h_row2: [0.0, 0.0, 1.0, 0.0],
                use_homography: if is_corner_pin { 1.0 } else { 0.0 },
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Vertex Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Warp Index Buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            params_buffer,
            vertex_buffer,
            index_buffer,
            num_indices: indices.len() as u32,
            is_corner_pin,
        }
    }

    /// Update the homography for corner-pin mode.
    pub fn set_homography(&mut self, queue: &wgpu::Queue, homography: &[f32; 12]) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&WarpParamsUniform {
                h_row0: [homography[0], homography[1], homography[2], 0.0],
                h_row1: [homography[3], homography[4], homography[5], 0.0],
                h_row2: [homography[6], homography[7], homography[8], 0.0],
                use_homography: 1.0,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            }),
        );
    }
}

impl ProjectionStage for WarpStage {
    fn label(&self) -> &str {
        "warp"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Warp Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Warp Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw_indexed(0..self.num_indices, 0, 0..1);
    }
}

fn build_mesh_buffers(mesh: &WarpMesh) -> (Vec<WarpVertex>, Vec<u16>) {
    let vertices: Vec<WarpVertex> = mesh
        .points
        .iter()
        .map(|p| WarpVertex {
            position: p.position,
            uv: p.uv,
        })
        .collect();

    let mut indices = Vec::new();
    let cols = mesh.cols as usize;
    let rows = mesh.rows as usize;
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let i0 = (r * cols + c) as u16;
            let i1 = i0 + 1;
            let i2 = ((r + 1) * cols + c) as u16;
            let i3 = i2 + 1;
            // Two triangles per cell
            indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_mesh_positions_equal_uvs() {
        let mesh = WarpMesh::identity(4, 4);
        assert_eq!(mesh.points.len(), 16);
        assert!(mesh.is_identity());
    }

    #[test]
    fn from_corners_creates_2x2_mesh() {
        let corners = [[0.1, 0.2], [0.9, 0.2], [0.9, 0.8], [0.1, 0.8]];
        let mesh = WarpMesh::from_corners(&corners);
        assert_eq!(mesh.cols, 2);
        assert_eq!(mesh.rows, 2);
        assert_eq!(mesh.points[0].position, corners[0]);
        assert_eq!(mesh.points[0].uv, [0.0, 0.0]);
    }

    #[test]
    fn xyuv_csv_roundtrip() {
        let mesh = WarpMesh::identity(3, 3);
        let csv = mesh.to_xyuv_csv();
        let parsed = WarpMesh::from_xyuv_csv(&csv).unwrap();
        assert_eq!(parsed.cols, 3);
        assert_eq!(parsed.rows, 3);
        assert!(parsed.is_identity());
    }

    #[test]
    fn identity_homography_is_identity_matrix() {
        let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let dst = src;
        let h = compute_forward_homography(&src, &dst);
        assert!((h[0] - 1.0).abs() < 1e-4);
        assert!((h[5] - 1.0).abs() < 1e-4);
        assert!((h[10] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn warp_mode_identity() {
        assert!(WarpMode::identity().is_identity());
        assert!(!WarpMode::corner_pin([[0.1, 0.0], [0.9, 0.0], [1.0, 1.0], [0.0, 1.0]]).is_identity());
    }
}
