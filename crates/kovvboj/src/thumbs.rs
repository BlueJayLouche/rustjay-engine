//! Per-layer thumbnails.
//!
//! Each layer owns a small render target that its output is blitted into once
//! per frame, and egui draws that. The alternative — the engine's own preview
//! path — registers a full-resolution texture and copies into it, which is
//! affordable for two or three previews and not for one per layer.
//!
//! The texture is ours, so the registered id never goes stale when a channel's
//! output ping-pongs between buffers: only the blit's *source* changes.

use std::collections::HashMap;

use rustjay_mixer::blit::BlitPipeline;
use rustjay_mixer::Mixer;

/// Small enough to cost nothing, large enough to tell two clips apart.
const THUMB_W: u32 = 160;
const THUMB_H: u32 = 90;

pub const ASPECT: f32 = THUMB_W as f32 / THUMB_H as f32;

struct Entry {
    #[allow(dead_code)] // Keeps the texture alive for the registered view.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    key: u64,
    registered: bool,
}

#[derive(Default)]
pub struct Thumbnails {
    pipeline: Option<BlitPipeline>,
    entries: HashMap<String, Entry>,
    next_key: u64,
    /// Resolved egui ids by layer uuid, refreshed by [`Self::sync`].
    pub ids: HashMap<String, egui::TextureId>,
}

impl Thumbnails {
    /// Blit every layer's current output into its thumbnail. Call after the
    /// mixer has rendered, or the outputs are a frame stale.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        vertex_buffer: &wgpu::Buffer,
        mixer: &Mixer,
    ) {
        let pipeline = self
            .pipeline
            .get_or_insert_with(|| BlitPipeline::new(device, wgpu::TextureFormat::Bgra8Unorm));

        for channel in &mixer.channels {
            let Some(source) = channel.output_texture() else {
                continue;
            };
            if !self.entries.contains_key(&channel.uuid) {
                self.next_key += 1;
                let key = self.next_key;
                let entry = {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("layer thumbnail"),
                        size: wgpu::Extent3d {
                            width: THUMB_W,
                            height: THUMB_H,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Bgra8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    Entry {
                        texture,
                        view,
                        key,
                        registered: false,
                    }
                };
                self.entries.insert(channel.uuid.clone(), entry);
            }
            let entry = &self.entries[&channel.uuid];
            // A blit samples, so this downscales; a plain texture copy would
            // take the top-left 160x90 corner instead.
            pipeline.blit(device, encoder, &source.view, &entry.view, vertex_buffer);
        }

        // Deleted layers must not keep a texture (or a registered id) alive.
        self.entries
            .retain(|uuid, _| mixer.channels.iter().any(|c| &c.uuid == uuid));
    }

    /// Register anything new and refresh [`Self::ids`]. Only the shell is
    /// handed the host, so it does this once a frame on everyone's behalf.
    pub fn sync(&mut self, host: &mut rustjay_gui::EguiControlGui) {
        for entry in self.entries.values_mut() {
            if !entry.registered {
                entry.registered = true;
                host.pending_textures.push((entry.key, entry.view.clone()));
            }
        }
        self.ids.clear();
        for (uuid, entry) in &self.entries {
            // Absent until the host settles the request at the end of the
            // frame the layer was created in, so a new layer shows its
            // placeholder for one frame.
            if let Some(id) = host.registered_textures.get(&entry.key) {
                self.ids.insert(uuid.clone(), *id);
            }
        }
    }
}
