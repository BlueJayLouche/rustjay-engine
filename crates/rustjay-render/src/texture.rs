//! # Texture Utilities
//!
//! Helper types for wgpu texture management.
//! All textures use BGRA8 format for native macOS compatibility.

use std::sync::Arc;

/// A GPU texture with its view, sampler, and dimensions.
pub struct Texture {
    /// Underlying wgpu texture.
    pub texture: wgpu::Texture,
    /// Default view of the texture.
    pub view: wgpu::TextureView,
    /// Sampler for the texture.
    pub sampler: wgpu::Sampler,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Texture {
    /// Wrap an existing wgpu texture.
    pub fn from_wgpu_texture(
        texture: wgpu::Texture,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self { texture, view, sampler, width, height }
    }

    /// Create a texture from raw BGRA pixel data.
    pub fn from_bgra(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        label: &str,
        data: &[u8],
    ) -> Self {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self { texture, view, sampler, width, height }
    }

    /// Create a render-target texture with the given dimensions.
    pub fn create_render_target(device: &wgpu::Device, width: u32, height: u32, label: &str) -> Self {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self { texture, view, sampler, width, height }
    }

    /// Update the texture contents from raw pixel data.
    pub fn update(&self, queue: &wgpu::Queue, data: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Fill the texture with black.
    pub fn clear_to_black(&self, queue: &wgpu::Queue) {
        let black = vec![0u8; (self.width * self.height * 4) as usize];
        self.update(queue, &black);
    }
}

/// Wrapper that manages engine input textures, including external sources.
pub struct InputTexture {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// Owned texture, if any.
    pub texture: Option<Texture>,
    has_data: bool,
    ext_view: Option<wgpu::TextureView>,
    ext_sampler: Option<wgpu::Sampler>,
    /// Generation counter bumped when the texture changes.
    pub texture_generation: u64,
}

impl InputTexture {
    /// Create a new input texture manager.
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            device,
            queue,
            texture: None,
            has_data: false,
            ext_view: None,
            ext_sampler: None,
            texture_generation: 0,
        }
    }

    /// Ensure the backing texture exists at the given size.
    pub fn ensure_size(&mut self, width: u32, height: u32) {
        match &self.texture {
            Some(tex) if tex.width == width && tex.height == height => {}
            _ => {
                log::info!("Creating input texture: {}x{}", width, height);
                self.texture = Some(Texture::from_bgra(
                    &self.device,
                    &self.queue,
                    width,
                    height,
                    "Input Texture",
                    &vec![0u8; (width * height * 4) as usize],
                ));
                self.texture_generation += 1;
            }
        }
    }

    /// Upload raw pixel data, resizing if necessary.
    pub fn update(&mut self, data: &[u8], width: u32, height: u32) {
        if self.ext_view.is_some() {
            self.ext_view = None;
            self.ext_sampler = None;
        }
        self.ensure_size(width, height);
        if let Some(ref tex) = self.texture {
            tex.update(&self.queue, data);
            self.has_data = true;
        }
    }

    /// Take ownership of an existing wgpu texture.
    pub fn swap_texture(&mut self, source: wgpu::Texture) {
        let width = source.width();
        let height = source.height();
        self.texture = Some(Texture::from_wgpu_texture(source, &self.device, width, height));
        self.has_data = true;
        self.texture_generation += 1;
    }

    /// Copy contents from another wgpu texture.
    pub fn update_from_texture(&mut self, source: &wgpu::Texture) {
        let width = source.width();
        let height = source.height();
        self.ensure_size(width, height);
        if let Some(ref dest) = self.texture {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Input Texture Copy"),
            });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: source,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &dest.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
            self.queue.submit(std::iter::once(encoder.finish()));
            self.has_data = true;
        }
    }

    /// Use an external texture as the input source.
    pub fn set_external_texture(&mut self, tex: &wgpu::Texture) {
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.ext_view = Some(view);
        self.ext_sampler = Some(sampler);
        self.has_data = true;
        self.texture_generation += 1;
    }

    /// Clear the external texture reference.
    pub fn clear_external_texture(&mut self) {
        self.ext_view = None;
        self.ext_sampler = None;
        self.texture_generation += 1;
    }

    /// Return the texture view to bind in shaders.
    pub fn binding_view(&self) -> Option<&wgpu::TextureView> {
        self.ext_view.as_ref()
            .or_else(|| self.texture.as_ref().map(|t| &t.view))
    }

    /// Return the sampler to bind in shaders.
    pub fn binding_sampler(&self) -> Option<&wgpu::Sampler> {
        self.ext_sampler.as_ref()
            .or_else(|| self.texture.as_ref().map(|t| &t.sampler))
    }

    /// Return the owned texture's view, if any.
    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.texture.as_ref().map(|t| &t.view)
    }

    /// Whether an external texture is currently set.
    pub fn has_external_texture(&self) -> bool {
        self.ext_view.is_some()
    }

    /// Whether any input data has been provided.
    pub fn has_data(&self) -> bool {
        self.has_data
    }

    /// Return the resolution of the input texture, defaulting to 1920x1080.
    pub fn resolution(&self) -> (u32, u32) {
        self.texture
            .as_ref()
            .map(|t| (t.width, t.height))
            .unwrap_or((1920, 1080))
    }
}

/// Texture that holds the previous frame's output for feedback effects.
pub struct PreviousFrameTexture {
    /// The underlying texture.
    pub texture: Texture,
}

impl PreviousFrameTexture {
    /// Create a new feedback texture with the given dimensions.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            texture: Texture::create_render_target(device, width, height, "Previous Frame"),
        }
    }

    /// Resize the feedback texture.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.texture = Texture::create_render_target(device, width, height, "Previous Frame");
    }

    /// Copy the contents of `source` into this feedback texture.
    pub fn copy_from(&self, encoder: &mut wgpu::CommandEncoder, source: &wgpu::Texture) {
        let width = self.texture.width.min(source.width());
        let height = self.texture.height.min(source.height());
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}
