#![cfg(target_arch = "wasm32")]

//! RustJay Delta — WebGPU + WASM + React
//!
//! Self-contained `cdylib` that runs the simplified delta effect in the
//! browser.  No native engine crates are used; only `wgpu` (WebGPU backend)
//! and `wasm-bindgen` for JS interop.

use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlCanvasElement;
use wgpu::util::DeviceExt;
use std::cell::RefCell;
use std::rc::Rc;

mod delta;
mod webcam;

use delta::{DeltaUniforms, create_pipeline};

// ---------------------------------------------------------------------------
// Parameter state (shared between JS setters and render loop)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
pub struct Params {
    pub delay_r: i32,
    pub delay_g: i32,
    pub delay_b: i32,
    pub mix_amount: f32,
}

thread_local! {
    static PARAMS: RefCell<Params> = RefCell::new(Params {
        delay_r: 0,
        delay_g: 5,
        delay_b: 10,
        mix_amount: 0.5,
    });
    static APP: RefCell<Option<App>> = RefCell::new(None);
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct App {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    pub webcam_texture: wgpu::Texture,
    pub webcam_view: wgpu::TextureView,
    pub feedback_texture: wgpu::Texture,
    pub feedback_view: wgpu::TextureView,
    pub texture_bind_group: wgpu::BindGroup,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Initialise the WebGPU context, create resources, and start the render loop.
#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas = document
        .get_element_by_id("canvas")
        .ok_or("no canvas element")?
        .dyn_into::<HtmlCanvasElement>()?;

    log::info!("Starting RustJay Web Delta");

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    let width = canvas.width();
    let height = canvas.height();

    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| JsValue::from_str(&format!("create_surface failed: {:?}", e)))?;

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("request_adapter failed: {:?}", e)))?;

    log::info!("Adapter: {:?}", adapter.get_info());

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            label: Some("Device"),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("request_device failed: {:?}", e)))?;

    // Returns the linear (non-sRGB) equivalent of a format so the feedback
    // texture can be sampled without gamma double-conversion, while remaining
    // copy-compatible with the (potentially sRGB) surface.
    fn strip_srgb(fmt: wgpu::TextureFormat) -> wgpu::TextureFormat {
        match fmt {
            wgpu::TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8Unorm,
            other => other,
        }
    }

    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(surface_caps.formats[0]);

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        format: surface_format,
        width,
        height,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: surface_caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    // --- Textures ----------------------------------------------------------

    let webcam_texture = device.create_texture(&wgpu::TextureDescriptor {
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
    let webcam_view = webcam_texture.create_view(&Default::default());

    let feedback_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("feedback"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: strip_srgb(surface_format),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let feedback_view = feedback_texture.create_view(&Default::default());

    // --- Pipeline & bind groups --------------------------------------------

    let (pipeline, texture_bind_group_layout, uniform_buffer, uniform_bind_group) =
        create_pipeline(&device, surface_format);

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture_bg"),
        layout: &texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&webcam_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&feedback_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    // --- Fullscreen quad ---------------------------------------------------

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Vertex {
        position: [f32; 2],
        texcoord: [f32; 2],
    }

    let vertices = [
        Vertex {
            position: [-1.0, -1.0],
            texcoord: [0.0, 1.0],
        },
        Vertex {
            position: [1.0, -1.0],
            texcoord: [1.0, 1.0],
        },
        Vertex {
            position: [1.0, 1.0],
            texcoord: [1.0, 0.0],
        },
        Vertex {
            position: [-1.0, -1.0],
            texcoord: [0.0, 1.0],
        },
        Vertex {
            position: [1.0, 1.0],
            texcoord: [1.0, 0.0],
        },
        Vertex {
            position: [-1.0, 1.0],
            texcoord: [0.0, 0.0],
        },
    ];

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vertex_buffer"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    // --- Store app state ---------------------------------------------------

    let app = App {
        device,
        queue,
        surface,
        config,
        pipeline,
        vertex_buffer,
        uniform_buffer,
        uniform_bind_group,
        webcam_texture,
        webcam_view,
        feedback_texture,
        feedback_view,
        texture_bind_group,
        texture_bind_group_layout,
    };

    APP.with(|a| {
        *a.borrow_mut() = Some(app);
    });

    // --- Render loop (RAF) -------------------------------------------------

    let win_for_closure = window.clone();
    let f = Rc::new(RefCell::new(None as Option<Closure<dyn FnMut()>>));
    let g = f.clone();

    *g.borrow_mut() = Some(Closure::new(move || {
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                render_frame(app);
            }
        });

        if let Some(closure) = f.borrow().as_ref() {
            let _ = win_for_closure
                .request_animation_frame(closure.as_ref().unchecked_ref());
        }
    }));

    if let Some(closure) = g.borrow().as_ref() {
        let _ = window.request_animation_frame(closure.as_ref().unchecked_ref());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Per-frame render
// ---------------------------------------------------------------------------

fn render_frame(app: &mut App) {
    let params = PARAMS.with(|p| *p.borrow());

    let surface_texture = match app.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
        other => {
            log::warn!("Surface issue: {:?}", other);
            return;
        }
    };

    let surface_view = surface_texture.texture.create_view(&Default::default());

    // Upload uniforms
    let uniforms =
        DeltaUniforms::new(params, app.config.width as f32, app.config.height as f32);
    app.queue
        .write_buffer(&app.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

    let mut encoder = app
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render_encoder"),
        });

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &surface_view,
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

        pass.set_pipeline(&app.pipeline);
        pass.set_vertex_buffer(0, app.vertex_buffer.slice(..));
        pass.set_bind_group(0, &app.texture_bind_group, &[]);
        pass.set_bind_group(1, &app.uniform_bind_group, &[]);
        pass.draw(0..6, 0..1);
    }

    // Copy render output into feedback texture for next frame
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &surface_texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &app.feedback_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: app.config.width,
            height: app.config.height,
            depth_or_array_layers: 1,
        },
    );

    app.queue.submit(Some(encoder.finish()));
    surface_texture.present();
}

// ---------------------------------------------------------------------------
// WASM exports — called from React / JS
// ---------------------------------------------------------------------------

/// Set red channel pixel offset.
#[wasm_bindgen]
pub fn set_delay_r(v: i32) {
    PARAMS.with(|p| p.borrow_mut().delay_r = v.clamp(-64, 64));
}

/// Set green channel pixel offset.
#[wasm_bindgen]
pub fn set_delay_g(v: i32) {
    PARAMS.with(|p| p.borrow_mut().delay_g = v.clamp(-64, 64));
}

/// Set blue channel pixel offset.
#[wasm_bindgen]
pub fn set_delay_b(v: i32) {
    PARAMS.with(|p| p.borrow_mut().delay_b = v.clamp(-64, 64));
}

/// Set blend factor between live webcam and delayed feedback.
#[wasm_bindgen]
pub fn set_mix(v: f32) {
    PARAMS.with(|p| p.borrow_mut().mix_amount = v);
}

/// Upload a new webcam frame (RGBA, `width * height * 4` bytes).
#[wasm_bindgen]
pub fn update_webcam_frame(data: &[u8], width: u32, height: u32) {
    const MAX_DIM: u32 = 4096;
    let width = width.min(MAX_DIM);
    let height = height.min(MAX_DIM);
    APP.with(|app| {
        if let Some(app) = app.borrow_mut().as_mut() {
            webcam::update_webcam(app, data, width, height);
        }
    });
}
