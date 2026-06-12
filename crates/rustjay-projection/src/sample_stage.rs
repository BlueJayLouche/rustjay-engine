//! `SampleStage` — packs multiple segment tiles into one sample atlas.
//!
//! Each segment defines a normalized source region (`u0,v0,u1,v1`) and a fixture
//! grid (`cols × rows`). The stage renders every segment into its own atlas tile
//! in a single render pass, using per-tile viewports and UV crops. The resulting
//! atlas is read back by [`PixelSampler`](super::pixel_sampler::PixelSampler) and
//! demuxed into DMX fixtures on the CPU.

use crate::identity::{BlitPipeline, BlitVertex};
use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

/// One tile inside a sample atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasTile {
    /// Top-left corner of the tile in the atlas, in pixels.
    pub offset: [u32; 2],
    /// Tile size in pixels (`[cols, rows]`).
    pub size: [u32; 2],
    /// Normalized source region to sample (`[u0, v0, u1, v1]`).
    pub region: [f32; 4],
}

/// Layout of a packed sample atlas for one lighting output.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasLayout {
    /// Total atlas size in pixels.
    pub size: [u32; 2],
    /// Tiles in atlas order.
    pub tiles: Vec<AtlasTile>,
}

impl AtlasLayout {
    /// Create a horizontal-strip atlas from segment descriptions.
    ///
    /// `segments` yields `(cols, rows, region)` for each segment. Tiles are laid
    /// left-to-right; atlas height is the maximum row count. This is simple and
    /// predictable — shelf packing can be swapped in later without API changes.
    pub fn from_segments(
        segments: impl IntoIterator<Item = ([u32; 2], [f32; 4])>,
    ) -> Self {
        let mut tiles = Vec::new();
        let mut x = 0u32;
        let mut height = 0u32;
        for (size, region) in segments {
            let [cols, rows] = [size[0].max(1), size[1].max(1)];
            tiles.push(AtlasTile {
                offset: [x, 0],
                size: [cols, rows],
                region,
            });
            x += cols;
            height = height.max(rows);
        }
        Self {
            size: [x.max(1), height.max(1)],
            tiles,
        }
    }

    /// Empty atlas containing a single 1×1 black pixel.
    pub fn empty() -> Self {
        Self {
            size: [1, 1],
            tiles: Vec::new(),
        }
    }
}

/// GPU draw state for one atlas tile.
struct TileDraw {
    viewport: [f32; 4],
    uniform_buffer: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
}

/// Renders an arbitrary number of segment regions into a packed atlas.
pub struct SampleStage {
    blit: BlitPipeline,
    vertex_buffer: wgpu::Buffer,
    layout: AtlasLayout,
    tiles: Vec<TileDraw>,
    cached_input_ptr: Option<usize>,
}

impl SampleStage {
    /// Create a new sample stage for the given atlas layout.
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        layout: AtlasLayout,
    ) -> Self {
        let blit = BlitPipeline::new(device, target_format);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SampleStage Vertex Buffer"),
            contents: bytemuck::cast_slice(&FULLSCREEN_QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut stage = Self {
            blit,
            vertex_buffer,
            layout: AtlasLayout::empty(),
            tiles: Vec::new(),
            cached_input_ptr: None,
        };
        stage.set_layout(device, layout);
        stage
    }

    /// Replace the atlas layout (and recreate per-tile GPU state).
    pub fn set_layout(&mut self, device: &wgpu::Device, layout: AtlasLayout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.tiles.clear();
        self.cached_input_ptr = None;
        for tile in &self.layout.tiles {
            self.tiles.push(TileDraw {
                viewport: [
                    tile.offset[0] as f32,
                    tile.offset[1] as f32,
                    tile.size[0] as f32,
                    tile.size[1] as f32,
                ],
                uniform_buffer: tile_uniform_buffer(device, tile.region),
                bind_group: None,
            });
        }
    }

    /// Current atlas layout.
    pub fn layout(&self) -> &AtlasLayout {
        &self.layout
    }
}

impl ProjectionStage for SampleStage {
    fn label(&self) -> &str {
        "sample"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        let input_ptr = input as *const _ as usize;
        if self.cached_input_ptr != Some(input_ptr) {
            for tile in &mut self.tiles {
                tile.bind_group = Some(self.blit.create_bind_group_nearest_with_uniform(
                    ctx.device,
                    input,
                    &tile.uniform_buffer,
                ));
            }
            self.cached_input_ptr = Some(input_ptr);
        }

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("SampleStage Atlas Pass"),
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

        pass.set_pipeline(&self.blit.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for tile in &self.tiles {
            pass.set_viewport(
                tile.viewport[0],
                tile.viewport[1],
                tile.viewport[2],
                tile.viewport[3],
                0.0,
                1.0,
            );
            if let Some(bg) = tile.bind_group.as_ref() {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_input_ptr = None;
    }
}

const FULLSCREEN_QUAD: [BlitVertex; 6] = [
    BlitVertex {
        position: [-1.0, -1.0],
        texcoord: [0.0, 1.0],
    },
    BlitVertex {
        position: [1.0, -1.0],
        texcoord: [1.0, 1.0],
    },
    BlitVertex {
        position: [-1.0, 1.0],
        texcoord: [0.0, 0.0],
    },
    BlitVertex {
        position: [-1.0, 1.0],
        texcoord: [0.0, 0.0],
    },
    BlitVertex {
        position: [1.0, -1.0],
        texcoord: [1.0, 1.0],
    },
    BlitVertex {
        position: [1.0, 1.0],
        texcoord: [1.0, 0.0],
    },
];

fn tile_uniform_buffer(device: &wgpu::Device, region: [f32; 4]) -> wgpu::Buffer {
    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct BlitParams {
        uv_scale: [f32; 2],
        uv_offset: [f32; 2],
        uv_crop: [f32; 4],
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("SampleStage Tile Params"),
        contents: bytemuck::cast_slice(&[BlitParams {
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
            uv_crop: region,
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}
