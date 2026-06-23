use wgpu::{Device, Queue, TextureFormat};

/// A decoded video frame ready for GPU upload.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA8
    pub pts: f64,      // seconds
}

impl VideoFrame {
    pub fn new(width: u32, height: u32, data: Vec<u8>, pts: f64) -> Self {
        debug_assert_eq!(data.len(), (width * height * 4) as usize);
        Self {
            width,
            height,
            data,
            pts,
        }
    }
}

/// Double-buffered RGBA texture for video frames.
pub struct Texture {
    textures: [wgpu::Texture; 2],
    bind_groups: [wgpu::BindGroup; 2],
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    current: usize,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    pub fn new(device: &Device, width: u32, height: u32) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video-texture-bind-group-layout"),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let make_texture = |device: &Device, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };

        let textures = [
            make_texture(device, "video-texture-0"),
            make_texture(device, "video-texture-1"),
        ];

        let make_bind_group = |device: &Device, texture: &wgpu::Texture, label: &str| {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };

        let bind_groups = [
            make_bind_group(device, &textures[0], "video-bind-group-0"),
            make_bind_group(device, &textures[1], "video-bind-group-1"),
        ];

        Self {
            textures,
            bind_groups,
            sampler,
            bind_group_layout,
            current: 0,
            width,
            height,
        }
    }

    /// Upload a decoded frame to the *back* texture and swap.
    pub fn upload(&mut self, queue: &Queue, frame: &VideoFrame) {
        let back = 1 - self.current;
        let texture = &self.textures[back];

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );

        self.current = back;
    }

    /// The currently-displayed bind group.
    pub fn current_bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_groups[self.current]
    }

    /// The bind group layout shared by both textures.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }
}
