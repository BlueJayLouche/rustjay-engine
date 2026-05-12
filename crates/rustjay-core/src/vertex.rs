//! # Vertex Types
//!
//! Vertex data structures for GPU rendering.

use bytemuck::{Pod, Zeroable};

/// Full-screen quad vertex
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    /// Position in normalized device coordinates (-1 to 1)
    pub position: [f32; 2],
    /// Texture coordinates (0 to 1)
    pub texcoord: [f32; 2],
}

impl Vertex {
    /// Create a full-screen quad (two triangles)
    /// Order: top-left, top-right, bottom-left, bottom-left, top-right, bottom-right
    pub fn quad_vertices() -> [Self; 6] {
        [
            // First triangle (top-left, top-right, bottom-left)
            Vertex { position: [-1.0, 1.0], texcoord: [0.0, 0.0] },
            Vertex { position: [1.0, 1.0], texcoord: [1.0, 0.0] },
            Vertex { position: [-1.0, -1.0], texcoord: [0.0, 1.0] },
            // Second triangle (bottom-left, top-right, bottom-right)
            Vertex { position: [-1.0, -1.0], texcoord: [0.0, 1.0] },
            Vertex { position: [1.0, 1.0], texcoord: [1.0, 0.0] },
            Vertex { position: [1.0, -1.0], texcoord: [1.0, 1.0] },
        ]
    }

    /// Vertex buffer layout descriptor
    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                // Position
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                // Texcoord
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}
