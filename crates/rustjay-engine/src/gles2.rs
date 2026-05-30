//! Native GLES 2.0 render path for hardware without GLES 3.0 / UBO support (e.g. Pi 2 VC4).
//!
//! Two backends:
//!   - **Wayland** (`gles2` feature) — renders into a weston window via EGL.
//!   - **DRM/GBM** (`drm-gles2` feature) — renders directly to the display via KMS/GBM;
//!     no compositor required.  This is the openFrameworks/kiosk-style path.

use anyhow::Result;
use rustjay_core::EngineState;
use std::sync::{Arc, Mutex};

// ── Public trait ──────────────────────────────────────────────────────────────

/// Implemented by effects that render via a native GLES 2.0 context.
pub trait Gles2Effect: Send + 'static {
    /// Called once after the GLES 2.0 context is ready.
    fn init_gl(&mut self, gl: &glow::Context, width: u32, height: u32, state: &EngineState) -> Result<()>;
    /// Called every frame. Return `false` to exit.
    fn render_frame(&mut self, gl: &glow::Context, state: &EngineState) -> Result<bool>;
    fn on_resize(&mut self, _gl: &glow::Context, _w: u32, _h: u32) {}
}

// ── Type-erased wrapper ───────────────────────────────────────────────────────

pub(crate) trait Gles2EffectDyn: Send + 'static {
    fn init_gl(&mut self, gl: &glow::Context, w: u32, h: u32, s: &EngineState) -> Result<()>;
    fn render_frame(&mut self, gl: &glow::Context, s: &EngineState) -> Result<bool>;
    fn on_resize(&mut self, gl: &glow::Context, w: u32, h: u32);
}
impl<G: Gles2Effect> Gles2EffectDyn for G {
    fn init_gl(&mut self, gl: &glow::Context, w: u32, h: u32, s: &EngineState) -> Result<()> { Gles2Effect::init_gl(self, gl, w, h, s) }
    fn render_frame(&mut self, gl: &glow::Context, s: &EngineState) -> Result<bool> { Gles2Effect::render_frame(self, gl, s) }
    fn on_resize(&mut self, gl: &glow::Context, w: u32, h: u32) { Gles2Effect::on_resize(self, gl, w, h); }
}

// ── Shared EGL helpers ────────────────────────────────────────────────────────

fn build_egl_instance() -> Result<Arc<khronos_egl::DynamicInstance<khronos_egl::EGL1_4>>> {
    use khronos_egl as egl;
    let inst = unsafe {
        egl::DynamicInstance::<egl::EGL1_4>::load_required()
            .map_err(|e| anyhow::anyhow!("Failed to load libEGL: {e}"))?
    };
    Ok(Arc::new(inst))
}

fn egl_init_and_config(
    egl: &khronos_egl::DynamicInstance<khronos_egl::EGL1_4>,
    display: khronos_egl::Display,
) -> Result<khronos_egl::Config> {
    use khronos_egl as egl_crate;
    egl.initialize(display).map_err(|e| anyhow::anyhow!("eglInitialize: {e}"))?;
    egl.bind_api(egl_crate::OPENGL_ES_API).map_err(|e| anyhow::anyhow!("eglBindAPI: {e}"))?;
    let attribs = [
        egl_crate::SURFACE_TYPE,    egl_crate::WINDOW_BIT as i32,
        egl_crate::RENDERABLE_TYPE, egl_crate::OPENGL_ES2_BIT as i32,
        egl_crate::RED_SIZE,   8,
        egl_crate::GREEN_SIZE, 8,
        egl_crate::BLUE_SIZE,  8,
        egl_crate::ALPHA_SIZE, 8,
        egl_crate::NONE,
    ];
    egl.choose_first_config(display, &attribs)
        .map_err(|e| anyhow::anyhow!("eglChooseConfig: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("No EGL config for GLES 2.0"))
}

fn egl_create_context(
    egl: &khronos_egl::DynamicInstance<khronos_egl::EGL1_4>,
    display: khronos_egl::Display,
    config: khronos_egl::Config,
) -> Result<khronos_egl::Context> {
    use khronos_egl as egl_crate;
    let attribs = [egl_crate::CONTEXT_MAJOR_VERSION, 2, egl_crate::NONE];
    egl.create_context(display, config, None, &attribs)
        .map_err(|e| anyhow::anyhow!("eglCreateContext (GLES 2.0): {e}"))
}

fn load_glow(egl: &khronos_egl::DynamicInstance<khronos_egl::EGL1_4>) -> glow::Context {
    unsafe {
        glow::Context::from_loader_function(|sym| {
            egl.get_proc_address(sym).map(|p| p as *const _).unwrap_or(std::ptr::null())
        })
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub(crate) struct Gles2State {
    pub(crate) egl:     Arc<khronos_egl::DynamicInstance<khronos_egl::EGL1_4>>,
    pub(crate) display: khronos_egl::Display,
    pub(crate) context: khronos_egl::Context,
    pub(crate) surface: khronos_egl::Surface,
    pub(crate) gl:      Arc<glow::Context>,
    pub(crate) width:   u32,
    pub(crate) height:  u32,

    // Wayland-specific (null in DRM mode)
    pub(crate) wl_egl_win: *mut wayland_sys::egl::wl_egl_window,

    // DRM/GBM-specific (None in Wayland mode)
    #[cfg(feature = "drm-gles2")]
    pub(crate) drm: Option<DrmState>,
}

#[cfg(feature = "drm-gles2")]
pub(crate) struct DrmState {
    pub(crate) gbm_dev:     gbm::Device<DrmCard>,
    pub(crate) gbm_surface: gbm::Surface<()>,
    pub(crate) crtc:       drm::control::crtc::Handle,
    pub(crate) connector:  drm::control::connector::Handle,
    pub(crate) mode:       drm::control::Mode,
    pub(crate) current_bo: Option<gbm::BufferObject<()>>,
    pub(crate) current_fb: Option<drm::control::framebuffer::Handle>,
    pub(crate) first_frame: bool,
}

impl Drop for Gles2State {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, Some(self.surface), Some(self.surface), Some(self.context));
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
        if !self.wl_egl_win.is_null() {
            unsafe {
                wayland_sys::ffi_dispatch!(
                    wayland_sys::egl::wayland_egl_handle(),
                    wl_egl_window_destroy,
                    self.wl_egl_win
                );
            }
        }
    }
}

impl Gles2State {
    /// Present the current frame. Wayland calls eglSwapBuffers; DRM additionally
    /// does a KMS page flip and waits for vblank.
    pub(crate) fn present(&mut self) -> Result<()> {
        self.egl.swap_buffers(self.display, self.surface)
            .map_err(|e| anyhow::anyhow!("eglSwapBuffers: {e}"))?;

        #[cfg(feature = "drm-gles2")]
        if let Some(ref mut drm) = self.drm {
            use drm::control::Device as DrmCtl;
            use gbm::AsRaw;
            use std::os::unix::io::AsRawFd;

            let new_bo = unsafe { drm.gbm_surface.lock_front_buffer() }
                .map_err(|e| anyhow::anyhow!("lock_front_buffer: {e:?}"))?;

            use std::os::unix::io::AsFd;
            let new_fb = DrmCtl::add_framebuffer(&*drm.gbm_dev, &new_bo, 24, 32)
                .map_err(|e| anyhow::anyhow!("add_framebuffer: {e}"))?;

            // Present via set_crtc every frame.
            // drmModePageFlip returns EBUSY on vc4/Pi 2 regardless of flags;
            // set_crtc is slower (no vblank sync) but reliably updates the display.
            DrmCtl::set_crtc(
                &*drm.gbm_dev,
                drm.crtc, Some(new_fb), (0, 0),
                &[drm.connector], Some(drm.mode),
            ).map_err(|e| anyhow::anyhow!("set_crtc: {e}"))?;
            drm.first_frame = false;

            if let Some(old_fb) = drm.current_fb.take() {
                let _ = DrmCtl::destroy_framebuffer(&*drm.gbm_dev, old_fb);
            }
            drm.current_bo = Some(new_bo);
            drm.current_fb = Some(new_fb);
        }

        Ok(())
    }
}

// ── Wayland EGL context creation ──────────────────────────────────────────────

pub(crate) fn try_create_gles2_context(
    wayland_display: *mut std::ffi::c_void,
    wayland_surface: *mut std::ffi::c_void,
    width: u32,
    height: u32,
) -> Result<Gles2State> {
    use khronos_egl as egl;

    let egl_inst = build_egl_instance()?;
    let display  = unsafe {
        egl_inst.get_display(wayland_display)
            .ok_or_else(|| anyhow::anyhow!("eglGetDisplay returned null"))?
    };
    let config  = egl_init_and_config(&egl_inst, display)?;
    let context = egl_create_context(&egl_inst, display, config)?;

    let wl_egl_win = unsafe {
        wayland_sys::ffi_dispatch!(
            wayland_sys::egl::wayland_egl_handle(),
            wl_egl_window_create,
            wayland_surface as *mut _,
            width as i32, height as i32
        )
    };
    if wl_egl_win.is_null() {
        return Err(anyhow::anyhow!("wl_egl_window_create returned null"));
    }

    let surface = unsafe {
        egl_inst.create_window_surface(display, config, wl_egl_win as egl::NativeWindowType, None)
            .map_err(|e| {
                wayland_sys::ffi_dispatch!(
                    wayland_sys::egl::wayland_egl_handle(),
                    wl_egl_window_destroy, wl_egl_win
                );
                anyhow::anyhow!("eglCreateWindowSurface: {e}")
            })?
    };

    egl_inst.make_current(display, Some(surface), Some(surface), Some(context))
        .map_err(|e| anyhow::anyhow!("eglMakeCurrent: {e}"))?;

    let gl = load_glow(&egl_inst);
    log::info!("GLES 2.0 Wayland context ready ({}×{})", width, height);

    Ok(Gles2State {
        egl: egl_inst, display, context, surface,
        gl: Arc::new(gl), width, height, wl_egl_win,
        #[cfg(feature = "drm-gles2")]
        drm: None,
    })
}

// ── DRM/GBM context creation ──────────────────────────────────────────────────

#[cfg(feature = "drm-gles2")]
const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;

/// Newtype that makes `std::fs::File` implement `drm::Device` and `drm::control::Device`.
#[cfg(feature = "drm-gles2")]
struct DrmCard(std::fs::File);

#[cfg(feature = "drm-gles2")]
impl std::os::unix::io::AsFd for DrmCard {
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> { self.0.as_fd() }
}
#[cfg(feature = "drm-gles2")]
impl drm::Device for DrmCard {}
#[cfg(feature = "drm-gles2")]
impl drm::control::Device for DrmCard {}

/// Create a GLES 2.0 EGL context that renders directly to the DRM display.
/// No compositor (weston/X11) is required.
#[cfg(feature = "drm-gles2")]
pub(crate) fn try_create_drm_gles2_context(drm_node: &str) -> Result<(Gles2State, u32, u32)> {
    use drm::control::{Device as DrmCtl, connector, crtc};
    use gbm::{AsRaw, BufferObjectFlags, Format};
    use khronos_egl as egl;
    use std::os::unix::io::{AsFd, AsRawFd};

    // ── 1. Open DRM device, wrap in DrmCard, then in GBM ──
    let file = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(drm_node)
        .map_err(|e| anyhow::anyhow!("Failed to open {drm_node}: {e}"))?;
    let card = DrmCard(file);
    let gbm_dev: gbm::Device<DrmCard> = gbm::Device::new(card)
        .map_err(|e| anyhow::anyhow!("gbm_create_device failed: {e}"))?;

    // ── 2. Find a connected output and preferred mode ──
    // gbm::Device<DrmCard> derefs to DrmCard, which implements drm::control::Device,
    // so all DRM/KMS operations can be called directly on gbm_dev.
    let resources = DrmCtl::resource_handles(&*gbm_dev)
        .map_err(|e| anyhow::anyhow!("drmModeGetResources: {e}"))?;

    let conn_info = resources.connectors().iter()
        .filter_map(|&h| DrmCtl::get_connector(&*gbm_dev, h, true).ok())
        .find(|c| c.state() == connector::State::Connected)
        .ok_or_else(|| anyhow::anyhow!("No connected DRM connector found"))?;

    let mode = *conn_info.modes().first()
        .ok_or_else(|| anyhow::anyhow!("Connector has no modes"))?;
    let (w, h) = (mode.size().0 as u32, mode.size().1 as u32);
    let connector_handle = conn_info.handle();

    // ── 3. Find a usable CRTC ──
    let crtc_handle = resources.crtcs().iter()
        .copied()
        .find(|&c| {
            DrmCtl::get_crtc(&*gbm_dev, c)
                .map(|info| info.mode().is_some())
                .unwrap_or(false)
        })
        .or_else(|| resources.crtcs().first().copied())
        .ok_or_else(|| anyhow::anyhow!("No usable CRTC found"))?;

    log::info!("DRM: {}×{} on connector {:?} crtc {:?}", w, h, connector_handle, crtc_handle);

    // ── 4. Create GBM surface ──
    let gbm_surface = gbm_dev.create_surface::<()>(
        w, h, Format::Xrgb8888,
        BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
    ).map_err(|e| anyhow::anyhow!("gbm_surface_create: {e}"))?;

    // ── 5. EGL: get GBM display ──
    let egl_inst = build_egl_instance()?;
    let gbm_dev_ptr = gbm_dev.as_raw() as *mut std::ffi::c_void;

    let display = {
        // Try EGL 1.5 get_platform_display (gives Mesa an unambiguous GBM platform hint)
        let egl5 = egl_inst.upcast::<khronos_egl::EGL1_5>();
        if let Some(ref e5) = egl5 {
            unsafe {
                e5.get_platform_display(
                    EGL_PLATFORM_GBM_KHR, gbm_dev_ptr,
                    &[khronos_egl::ATTRIB_NONE],
                ).map_err(|e| anyhow::anyhow!("eglGetPlatformDisplay(GBM): {e}"))?
            }
        } else {
            unsafe {
                egl_inst.get_display(gbm_dev_ptr)
                    .ok_or_else(|| anyhow::anyhow!("eglGetDisplay(GBM) returned null"))?
            }
        }
    };

    let config  = egl_init_and_config(&egl_inst, display)?;
    let context = egl_create_context(&egl_inst, display, config)?;

    let gbm_surf_ptr = gbm_surface.as_raw() as egl::NativeWindowType;
    let egl_surface = unsafe {
        egl_inst.create_window_surface(display, config, gbm_surf_ptr, None)
            .map_err(|e| anyhow::anyhow!("eglCreateWindowSurface(GBM): {e}"))?
    };

    egl_inst.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
        .map_err(|e| anyhow::anyhow!("eglMakeCurrent: {e}"))?;

    // Sync eglSwapBuffers to vblank so set_crtc is called at the right moment.
    let _ = egl_inst.swap_interval(display, 1);

    let gl = load_glow(&egl_inst);
    log::info!("GLES 2.0 DRM/GBM context ready ({}×{})", w, h);

    Ok((
        Gles2State {
            egl: egl_inst, display, context, surface: egl_surface,
            gl: Arc::new(gl), width: w, height: h,
            wl_egl_win: std::ptr::null_mut(),
            drm: Some(DrmState {
                gbm_dev, gbm_surface,
                crtc: crtc_handle, connector: connector_handle, mode,
                current_bo: None, current_fb: None,
                first_frame: true,
            }),
        },
        w, h,
    ))
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run using a Wayland-backed GLES 2.0 context (requires a running compositor).
pub fn run_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where P: rustjay_core::EffectPlugin, G: Gles2Effect,
{
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    crate::app::run_gles2_app(shared_state, plugin, Box::new(gles2), false)
}

/// Run using a DRM/GBM-backed GLES 2.0 context — no compositor required.
///
/// Opens `/dev/dri/card0` directly and renders fullscreen via KMS page flipping.
/// No compositor required. All engine services (OSC, MIDI, audio, Web UI) run normally.
#[cfg(feature = "drm-gles2")]
pub fn run_drm_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where P: rustjay_core::EffectPlugin, G: Gles2Effect,
{
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    run_drm_gles2_loop(shared_state, plugin, Box::new(gles2))
}

#[cfg(feature = "drm-gles2")]
fn run_drm_gles2_loop<P: rustjay_core::EffectPlugin>(
    shared_state: Arc<Mutex<EngineState>>,
    plugin: P,
    mut gles2: Box<dyn Gles2EffectDyn>,
) -> Result<()> {
    use rustjay_audio::AudioAnalyzer;
    use rustjay_control::{MidiManager, MidiState, OscServer};
    use rustjay_control::{WebServer, WebConfig, WebCommand as WebServerCommand};
    use rustjay_presets::{PresetBank, presets_dir_for};
    use crate::config::{AppSettings, ConfigManager};

    let app_name = plugin.app_name().to_string();
    let config_manager = ConfigManager::new(&app_name);

    // Apply saved config and cap fps for headless
    {
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        config_manager.settings.apply_to_state(&mut state);
        state.output_fullscreen = true;
        if state.target_fps > 30 { state.target_fps = 30; }
    }

    // Register effect parameters
    let descriptors = plugin.parameters();
    {
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        state.param_descriptors    = std::sync::Arc::new(descriptors.clone());
        state.hidden_tabs          = plugin.hidden_tabs();
        state.custom_param_bases.resize(descriptors.len(), 0.0);
        state.custom_params.resize(descriptors.len(), 0.0);
        for (i, d) in descriptors.iter().enumerate() {
            state.custom_param_bases[i] = d.default;
            state.custom_params[i]      = d.default;
        }
        state.param_osc_addresses = descriptors.iter()
            .map(|d| format!("/{}/{}", d.category.name().to_lowercase(), d.id))
            .collect();
    }

    // Audio
    let mut analyzer = AudioAnalyzer::new();
    let (fft_size, audio_dev) = {
        let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        (s.audio.fft_size, s.audio.selected_device.clone())
    };
    analyzer.set_fft_size(fft_size);
    match analyzer.start_with_device(audio_dev.as_deref()) {
        Ok(name) => { shared_state.lock().unwrap_or_else(|e| e.into_inner()).audio.selected_device = Some(name); }
        Err(e)   => log::warn!("Audio: {e}"),
    }

    // MIDI
    let midi_state = std::sync::Arc::new(Mutex::new(MidiState::default()));
    let midi_manager = MidiManager::new(midi_state).ok().map(|mut m| {
        let devs = m.refresh_devices();
        shared_state.lock().unwrap_or_else(|e| e.into_inner()).midi_available_devices = devs;
        m
    });

    // OSC
    let (osc_host, osc_port) = {
        let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        (s.osc_host.clone(), s.osc_port)
    };
    let mut osc_server = OscServer::new(&osc_host, osc_port, "/rustjay");
    if let Ok(mut st) = osc_server.state().lock() {
        st.register_default_parameters();
        st.register_parameters(&descriptors);
    }
    log::info!("OSC server initialized");

    // Web UI
    let (web_host, web_port, web_lan) = {
        let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        (s.web_host.clone(), s.web_port, s.web_lan_trust)
    };
    let (mut web_server, _web_tx) = WebServer::new(WebConfig {
        host: web_host.clone(), port: web_port,
        app_name: app_name.clone(), enabled: false, lan_trust: web_lan,
    });
    web_server.register_default_parameters();
    web_server.register_parameters(&descriptors);
    shared_state.lock().unwrap_or_else(|e| e.into_inner()).web_app_name = app_name.clone();
    if let Err(e) = web_server.start() {
        log::error!("Web UI failed to start: {e}");
    } else {
        log::info!("Web UI running at http://{}:{}", web_host, web_port);
    }

    // Presets
    if let Ok(dir) = presets_dir_for(&app_name) {
        let bank = PresetBank::new(dir);
        let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
        let slots: [Option<String>; 8] = std::array::from_fn(|i| bank.get_slot_name(i + 1).map(|s| s.to_string()));
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        state.preset_names            = names;
        state.preset_quick_slot_names = slots;
    }

    // DRM/GBM/EGL context
    let (mut gles2_state, w, h) = try_create_drm_gles2_context("/dev/dri/card0")?;
    {
        let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        gles2.init_gl(&gles2_state.gl, w, h, &state)?;
    }
    log::info!("DRM render loop starting at {w}×{h}");

    // Main render loop
    loop {
        let frame_start = std::time::Instant::now();

        // ── Poll engine services ──────────────────────────────────────────────
        // Audio: push latest FFT + volume into shared state
        {
            let fft    = analyzer.get_fft();
            let volume = analyzer.get_volume();
            let beat   = analyzer.is_beat();
            let phase  = analyzer.get_beat_phase();
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            analyzer.set_amplitude(state.audio.amplitude);
            analyzer.set_smoothing(state.audio.smoothing);
            if state.audio.enabled {
                state.audio.fft        = fft;
                state.audio.volume     = volume;
                state.audio.beat       = beat;
                state.audio.beat_phase = phase;
                state.reset_custom_params_to_base();
            }
        }

        // OSC: sync dirty parameter values into shared state
        if let Ok(mut osc_st) = osc_server.state().lock() {
            if let Ok(mut state) = shared_state.lock() {
                let descs = std::sync::Arc::clone(&state.param_descriptors);
                for (i, desc) in descs.iter().enumerate() {
                    if let Some(addr) = state.param_osc_addresses.get(i).cloned() {
                        if let Some(v) = osc_st.get_value_if_dirty(&addr) {
                            state.set_param_base(&desc.id, v.clamp(desc.min, desc.max));
                        }
                    }
                }
            }
        }

        // Web: apply parameter changes from web clients
        {
            while let Ok(cmd) = web_server.command_rx.try_recv() {
                match cmd {
                    WebServerCommand::Set { id, value } => {
                        if let Ok(mut state) = shared_state.lock() {
                            match id.as_str() {
                                "color/hue_shift" => {
                                    state.hsb_params.hue_shift = value.clamp(-180.0, 180.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/saturation" => {
                                    state.hsb_params.saturation = value.clamp(0.0, 2.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/brightness" => {
                                    state.hsb_params.brightness = value.clamp(0.0, 2.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/enabled" => state.color_enabled = value > 0.5,
                                "audio/amplitude" => state.audio.amplitude = value.clamp(0.0, 5.0),
                                "audio/smoothing" => state.audio.smoothing = value.clamp(0.0, 1.0),
                                "audio/enabled" => state.audio.enabled = value > 0.5,
                                "audio/normalize" => state.audio.normalize = value > 0.5,
                                "audio/pink_noise" => state.audio.pink_noise_shaping = value > 0.5,
                                "output/fullscreen" => state.output_fullscreen = value > 0.5,
                                _ => {
                                    if let Some(desc) = state.param_descriptors.iter().find(|d| {
                                        format!("{}/{}", d.category.name().to_lowercase(), d.id) == id
                                    }) {
                                        let (desc_id, desc_min, desc_max) = (desc.id.clone(), desc.min, desc.max);
                                        state.set_param_base(&desc_id, value.clamp(desc_min, desc_max));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let target_fps = shared_state.lock().unwrap_or_else(|e| e.into_inner()).target_fps;

        // Web: broadcast current parameter values to connected clients
        if web_server.is_running() {
            if let Ok(state) = shared_state.lock() {
                web_server.update_parameter("color/hue_shift", state.hsb_params.hue_shift);
                web_server.update_parameter("color/saturation", state.hsb_params.saturation);
                web_server.update_parameter("color/brightness", state.hsb_params.brightness);
                web_server.update_parameter("color/enabled", if state.color_enabled { 1.0 } else { 0.0 });
                web_server.update_parameter("audio/amplitude", state.audio.amplitude);
                web_server.update_parameter("audio/smoothing", state.audio.smoothing);
                web_server.update_parameter("audio/enabled", if state.audio.enabled { 1.0 } else { 0.0 });
                web_server.update_parameter("audio/normalize", if state.audio.normalize { 1.0 } else { 0.0 });
                web_server.update_parameter("audio/pink_noise", if state.audio.pink_noise_shaping { 1.0 } else { 0.0 });
                web_server.update_parameter("output/fullscreen", if state.output_fullscreen { 1.0 } else { 0.0 });
                let descriptors = Arc::clone(&state.param_descriptors);
                for (i, desc) in descriptors.iter().enumerate() {
                    if let Some(addr) = state.param_osc_addresses.get(i) {
                        let id = addr.trim_start_matches('/');
                        let value = state.get_param_base(&desc.id).unwrap_or(desc.default);
                        web_server.update_parameter(id, value);
                    }
                }
            }
        }

        // Render frame
        let gl = gles2_state.gl.clone();
        let keep = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            match gles2.render_frame(&gl, &state) {
                Ok(v)  => v,
                Err(e) => { log::error!("DRM render: {e}"); true }
            }
        };
        if !keep { break; }

        // Page flip (provides vsync)
        if let Err(e) = gles2_state.present() {
            log::error!("DRM present: {e}");
        }

        // Settings persist
        let should_save = {
            let mut s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            if s.save_settings_requested { s.save_settings_requested = false; true } else { false }
        };
        if should_save {
            let settings = AppSettings::from_state(
                &shared_state.lock().unwrap_or_else(|e| e.into_inner())
            );
            if let Err(e) = settings.save(&app_name) { log::error!("Save: {e}"); }
        }

        // Extra sleep only when page flip returns faster than the target rate
        let target_dur = std::time::Duration::from_micros(1_000_000 / target_fps.max(1) as u64);
        let elapsed = frame_start.elapsed();
        if elapsed < target_dur { std::thread::sleep(target_dur - elapsed); }
    }

    let settings = AppSettings::from_state(&shared_state.lock().unwrap_or_else(|e| e.into_inner()));
    let _ = settings.save(&app_name);
    analyzer.stop();
    osc_server.stop();
    log::info!("DRM shutdown complete");
    Ok(())
}
