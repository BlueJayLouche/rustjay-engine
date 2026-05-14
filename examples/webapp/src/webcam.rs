//! Webcam texture upload — called from JS each frame.

use crate::App;

/// Upload a raw RGBA frame into the webcam texture.
///
/// If `width`/`height` differ from the current texture, it is recreated
/// and the texture bind group is updated.
pub fn update_webcam(app: &mut App, data: &[u8], width: u32, height: u32) {
    if width == 0 || height == 0 {
        return;
    }

    // Recreate texture if dimensions changed
    if app.webcam_texture.width() != width || app.webcam_texture.height() != height {
        app.webcam_texture = app.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webcam"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        app.webcam_view = app.webcam_texture.create_view(&Default::default());

        let sampler = app.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        app.texture_bind_group = app.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture_bg"),
            layout: &app.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&app.webcam_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&app.feedback_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
    }

    app.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &app.webcam_texture,
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
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}
