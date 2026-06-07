//! Native GLES 2.0 render path for hardware without GLES 3.0 / UBO support (e.g. Pi 2 VC4).
//!
//! Two backends:
//!   - **Wayland** (`gles2` feature) — renders into a weston window via EGL.
//!   - **DRM/GBM** (`drm-gles2` feature) — renders directly to the display via KMS/GBM;
//!     no compositor required.  This is the openFrameworks/kiosk-style path.

use anyhow::Result;
use rustjay_control::InputWebCommand;
use rustjay_core::EngineState;
use std::sync::{Arc, Mutex};

// ── Public trait ──────────────────────────────────────────────────────────────

/// Implemented by effects that render via a native GLES 2.0 context.
pub trait Gles2Effect: Send + 'static {
    /// Called once after the GLES 2.0 context is ready.
    fn init_gl(
        &mut self,
        gl: &glow::Context,
        width: u32,
        height: u32,
        state: &EngineState,
    ) -> Result<()>;
    /// Called every frame. Return `false` to exit.
    fn render_frame(&mut self, gl: &glow::Context, state: &EngineState) -> Result<bool>;
    fn on_resize(&mut self, _gl: &glow::Context, _w: u32, _h: u32) {}
    /// Called from the run loop when an InputWebCommand arrives.
    /// Default implementation is a no-op so existing effects don't need to change.
    fn handle_input_command(&mut self, _gl: &glow::Context, _cmd: InputWebCommand) {}
    /// Return the current input state for web broadcast.
    /// Default implementation returns `None` so existing effects don't need to change.
    fn get_input_state(&self) -> Option<rustjay_control::InputStateJson> {
        None
    }
}

// ── Type-erased wrapper ───────────────────────────────────────────────────────

pub(crate) trait Gles2EffectDyn: Send + 'static {
    fn init_gl(&mut self, gl: &glow::Context, w: u32, h: u32, s: &EngineState) -> Result<()>;
    fn render_frame(&mut self, gl: &glow::Context, s: &EngineState) -> Result<bool>;
    fn on_resize(&mut self, gl: &glow::Context, w: u32, h: u32);
    fn handle_input_command(&mut self, gl: &glow::Context, cmd: InputWebCommand);
    fn get_input_state(&self) -> Option<rustjay_control::InputStateJson>;
}
impl<G: Gles2Effect> Gles2EffectDyn for G {
    fn init_gl(&mut self, gl: &glow::Context, w: u32, h: u32, s: &EngineState) -> Result<()> {
        Gles2Effect::init_gl(self, gl, w, h, s)
    }
    fn render_frame(&mut self, gl: &glow::Context, s: &EngineState) -> Result<bool> {
        Gles2Effect::render_frame(self, gl, s)
    }
    fn on_resize(&mut self, gl: &glow::Context, w: u32, h: u32) {
        Gles2Effect::on_resize(self, gl, w, h);
    }
    fn handle_input_command(&mut self, gl: &glow::Context, cmd: InputWebCommand) {
        Gles2Effect::handle_input_command(self, gl, cmd);
    }
    fn get_input_state(&self) -> Option<rustjay_control::InputStateJson> {
        Gles2Effect::get_input_state(self)
    }
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
    egl.initialize(display)
        .map_err(|e| anyhow::anyhow!("eglInitialize: {e}"))?;
    egl.bind_api(egl_crate::OPENGL_ES_API)
        .map_err(|e| anyhow::anyhow!("eglBindAPI: {e}"))?;
    let attribs = [
        egl_crate::SURFACE_TYPE,
        egl_crate::WINDOW_BIT as i32,
        egl_crate::RENDERABLE_TYPE,
        egl_crate::OPENGL_ES2_BIT as i32,
        egl_crate::RED_SIZE,
        8,
        egl_crate::GREEN_SIZE,
        8,
        egl_crate::BLUE_SIZE,
        8,
        egl_crate::ALPHA_SIZE,
        8,
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
            egl.get_proc_address(sym)
                .map(|p| p as *const _)
                .unwrap_or(std::ptr::null())
        })
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub(crate) struct Gles2State {
    pub(crate) egl: Arc<khronos_egl::DynamicInstance<khronos_egl::EGL1_4>>,
    pub(crate) display: khronos_egl::Display,
    pub(crate) context: khronos_egl::Context,
    pub(crate) surface: khronos_egl::Surface,
    pub(crate) gl: Arc<glow::Context>,
    pub(crate) width: u32,
    pub(crate) height: u32,

    // Wayland-specific (null in DRM mode)
    pub(crate) wl_egl_win: *mut wayland_sys::egl::wl_egl_window,

    // DRM/GBM-specific (None in Wayland mode)
    #[cfg(feature = "drm-gles2")]
    pub(crate) drm: Option<DrmState>,
}

#[cfg(feature = "drm-gles2")]
pub(crate) struct DrmState {
    pub(crate) gbm_dev: gbm::Device<DrmCard>,
    pub(crate) gbm_surface: gbm::Surface<()>,
    pub(crate) crtc: drm::control::crtc::Handle,
    pub(crate) connector: drm::control::connector::Handle,
    pub(crate) mode: drm::control::Mode,
    pub(crate) current_bo: Option<gbm::BufferObject<()>>,
    pub(crate) current_fb: Option<drm::control::framebuffer::Handle>,
    pub(crate) first_frame: bool,
}

impl Drop for Gles2State {
    fn drop(&mut self) {
        let _ = self.egl.make_current(
            self.display,
            Some(self.surface),
            Some(self.surface),
            Some(self.context),
        );
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
        self.egl
            .swap_buffers(self.display, self.surface)
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
                drm.crtc,
                Some(new_fb),
                (0, 0),
                &[drm.connector],
                Some(drm.mode),
            )
            .map_err(|e| anyhow::anyhow!("set_crtc: {e}"))?;
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
    let display = unsafe {
        egl_inst
            .get_display(wayland_display)
            .ok_or_else(|| anyhow::anyhow!("eglGetDisplay returned null"))?
    };
    let config = egl_init_and_config(&egl_inst, display)?;
    let context = egl_create_context(&egl_inst, display, config)?;

    let wl_egl_win = unsafe {
        wayland_sys::ffi_dispatch!(
            wayland_sys::egl::wayland_egl_handle(),
            wl_egl_window_create,
            wayland_surface as *mut _,
            width as i32,
            height as i32
        )
    };
    if wl_egl_win.is_null() {
        return Err(anyhow::anyhow!("wl_egl_window_create returned null"));
    }

    let surface = unsafe {
        egl_inst
            .create_window_surface(display, config, wl_egl_win as egl::NativeWindowType, None)
            .map_err(|e| {
                wayland_sys::ffi_dispatch!(
                    wayland_sys::egl::wayland_egl_handle(),
                    wl_egl_window_destroy,
                    wl_egl_win
                );
                anyhow::anyhow!("eglCreateWindowSurface: {e}")
            })?
    };

    egl_inst
        .make_current(display, Some(surface), Some(surface), Some(context))
        .map_err(|e| anyhow::anyhow!("eglMakeCurrent: {e}"))?;

    let gl = load_glow(&egl_inst);
    log::info!("GLES 2.0 Wayland context ready ({}×{})", width, height);

    Ok(Gles2State {
        egl: egl_inst,
        display,
        context,
        surface,
        gl: Arc::new(gl),
        width,
        height,
        wl_egl_win,
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
    fn as_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        self.0.as_fd()
    }
}
#[cfg(feature = "drm-gles2")]
impl drm::Device for DrmCard {}
#[cfg(feature = "drm-gles2")]
impl drm::control::Device for DrmCard {}

/// Create a GLES 2.0 EGL context that renders directly to the DRM display.
/// No compositor (weston/X11) is required.
#[cfg(feature = "drm-gles2")]
pub(crate) fn try_create_drm_gles2_context(drm_node: &str) -> Result<(Gles2State, u32, u32)> {
    use drm::control::{connector, crtc, Device as DrmCtl};
    use gbm::{AsRaw, BufferObjectFlags, Format};
    use khronos_egl as egl;
    use std::os::unix::io::{AsFd, AsRawFd};

    // ── 1. Open DRM device, wrap in DrmCard, then in GBM ──
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(drm_node)
        .map_err(|e| anyhow::anyhow!("Failed to open {drm_node}: {e}"))?;
    let card = DrmCard(file);
    let gbm_dev: gbm::Device<DrmCard> =
        gbm::Device::new(card).map_err(|e| anyhow::anyhow!("gbm_create_device failed: {e}"))?;

    // ── 2. Find a connected output and preferred mode ──
    // gbm::Device<DrmCard> derefs to DrmCard, which implements drm::control::Device,
    // so all DRM/KMS operations can be called directly on gbm_dev.
    let resources = DrmCtl::resource_handles(&*gbm_dev)
        .map_err(|e| anyhow::anyhow!("drmModeGetResources: {e}"))?;

    let conn_info = resources
        .connectors()
        .iter()
        .filter_map(|&h| DrmCtl::get_connector(&*gbm_dev, h, true).ok())
        .find(|c| c.state() == connector::State::Connected)
        .ok_or_else(|| anyhow::anyhow!("No connected DRM connector found"))?;

    let mode = *conn_info
        .modes()
        .first()
        .ok_or_else(|| anyhow::anyhow!("Connector has no modes"))?;
    let (w, h) = (mode.size().0 as u32, mode.size().1 as u32);
    let connector_handle = conn_info.handle();

    // ── 3. Find a usable CRTC ──
    let crtc_handle = resources
        .crtcs()
        .iter()
        .copied()
        .find(|&c| {
            DrmCtl::get_crtc(&*gbm_dev, c)
                .map(|info| info.mode().is_some())
                .unwrap_or(false)
        })
        .or_else(|| resources.crtcs().first().copied())
        .ok_or_else(|| anyhow::anyhow!("No usable CRTC found"))?;

    log::info!(
        "DRM: {}×{} on connector {:?} crtc {:?}",
        w,
        h,
        connector_handle,
        crtc_handle
    );

    // ── 4. Create GBM surface ──
    let gbm_surface = gbm_dev
        .create_surface::<()>(
            w,
            h,
            Format::Xrgb8888,
            BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
        )
        .map_err(|e| anyhow::anyhow!("gbm_surface_create: {e}"))?;

    // ── 5. EGL: get GBM display ──
    let egl_inst = build_egl_instance()?;
    let gbm_dev_ptr = gbm_dev.as_raw() as *mut std::ffi::c_void;

    let display = {
        // Try EGL 1.5 get_platform_display (gives Mesa an unambiguous GBM platform hint)
        let egl5 = egl_inst.upcast::<khronos_egl::EGL1_5>();
        if let Some(ref e5) = egl5 {
            unsafe {
                e5.get_platform_display(
                    EGL_PLATFORM_GBM_KHR,
                    gbm_dev_ptr,
                    &[khronos_egl::ATTRIB_NONE],
                )
                .map_err(|e| anyhow::anyhow!("eglGetPlatformDisplay(GBM): {e}"))?
            }
        } else {
            unsafe {
                egl_inst
                    .get_display(gbm_dev_ptr)
                    .ok_or_else(|| anyhow::anyhow!("eglGetDisplay(GBM) returned null"))?
            }
        }
    };

    let config = egl_init_and_config(&egl_inst, display)?;
    let context = egl_create_context(&egl_inst, display, config)?;

    let gbm_surf_ptr = gbm_surface.as_raw() as egl::NativeWindowType;
    let egl_surface = unsafe {
        egl_inst
            .create_window_surface(display, config, gbm_surf_ptr, None)
            .map_err(|e| anyhow::anyhow!("eglCreateWindowSurface(GBM): {e}"))?
    };

    egl_inst
        .make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
        .map_err(|e| anyhow::anyhow!("eglMakeCurrent: {e}"))?;

    // Sync eglSwapBuffers to vblank so set_crtc is called at the right moment.
    let _ = egl_inst.swap_interval(display, 1);

    let gl = load_glow(&egl_inst);
    log::info!("GLES 2.0 DRM/GBM context ready ({}×{})", w, h);

    Ok((
        Gles2State {
            egl: egl_inst,
            display,
            context,
            surface: egl_surface,
            gl: Arc::new(gl),
            width: w,
            height: h,
            wl_egl_win: std::ptr::null_mut(),
            drm: Some(DrmState {
                gbm_dev,
                gbm_surface,
                crtc: crtc_handle,
                connector: connector_handle,
                mode,
                current_bo: None,
                current_fb: None,
                first_frame: true,
            }),
        },
        w,
        h,
    ))
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run using a Wayland-backed GLES 2.0 context (requires a running compositor).
pub fn run_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where
    P: rustjay_core::EffectPlugin,
    G: Gles2Effect,
{
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    crate::app::run_gles2_app(shared_state, plugin, Box::new(gles2), false)
}

/// Convert a parameter id string to a ModulationTarget for audio routing.
#[cfg(feature = "drm-gles2")]
fn param_id_to_modulation_target(param_id: &str) -> rustjay_core::ModulationTarget {
    for t in rustjay_core::ModulationTarget::all() {
        if t.param_id() == Some(param_id) {
            return t.clone();
        }
    }
    rustjay_core::ModulationTarget::Custom(param_id.to_string())
}

/// Run using a DRM/GBM-backed GLES 2.0 context — no compositor required.
///
/// Opens `/dev/dri/card0` directly and renders fullscreen via KMS page flipping.
/// No compositor required. All engine services (OSC, MIDI, audio, Web UI) run normally.
#[cfg(feature = "drm-gles2")]
pub fn run_drm_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where
    P: rustjay_core::EffectPlugin,
    G: Gles2Effect,
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
    use crate::config::{AppSettings, ConfigManager};
    use rustjay_audio::AudioAnalyzer;
    use rustjay_control::{MidiManager, MidiState, OscServer};
    use rustjay_control::{WebCommand as WebServerCommand, WebConfig, WebServer};
    use rustjay_presets::{presets_dir_for, PresetBank};

    // On Pi with a read-only root filesystem, the default config dir (~/.config)
    // is unwritable. Detect this before loading config and redirect to the FAT32
    // boot partition, which remains writable under the RO/RW toggle scripts.
    // If XDG_CONFIG_HOME is already set (e.g. from the systemd unit), skip the probe.
    if std::env::var_os("XDG_CONFIG_HOME").is_none() {
        if let Some(cfg) = dirs::config_dir() {
            let probe_dir = cfg.join("rustjay");
            let _ = std::fs::create_dir_all(&probe_dir);
            let probe = probe_dir.join(".write_probe");
            match std::fs::write(&probe, b"") {
                Ok(_) => {
                    let _ = std::fs::remove_file(&probe);
                }
                Err(_) => {
                    let boot = "/boot/rustjay-data";
                    match std::fs::create_dir_all(boot) {
                        Ok(_) => {
                            // Safety: called before any threads are spawned in this loop.
                            unsafe {
                                std::env::set_var("XDG_CONFIG_HOME", boot);
                            }
                            log::info!(
                                "Config dir {:?} is read-only; redirecting saves to {}. \
                                 Set XDG_CONFIG_HOME={} in the systemd unit to make this permanent.",
                                cfg, boot, boot
                            );
                        }
                        Err(e) => {
                            log::error!(
                                "Config dir {:?} is read-only and {} could not be created ({}). \
                                 Preset and settings saves will fail this session. \
                                 Fix: mkdir -p {} && set XDG_CONFIG_HOME={} in flux.service.",
                                cfg,
                                boot,
                                e,
                                boot,
                                boot
                            );
                        }
                    }
                }
            }
        }
    }

    let app_name = plugin.app_name().to_string();
    let config_manager = ConfigManager::new(&app_name);

    // Apply saved config and cap fps for headless
    {
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        config_manager.settings.apply_to_state(&mut state);
        state.output_fullscreen = true;
        if state.target_fps > 30 {
            state.target_fps = 30;
        }
    }

    // Register effect parameters
    let descriptors = plugin.parameters();
    {
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        state.param_descriptors = std::sync::Arc::new(descriptors.clone());
        state.hidden_tabs = plugin.hidden_tabs();
        state.custom_param_bases.resize(descriptors.len(), 0.0);
        state.custom_params.resize(descriptors.len(), 0.0);
        for (i, d) in descriptors.iter().enumerate() {
            state.custom_param_bases[i] = d.default;
            state.custom_params[i] = d.default;
        }
        state.param_osc_addresses = descriptors
            .iter()
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
        Ok(name) => {
            shared_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .audio
                .selected_device = Some(name);
        }
        Err(e) => log::warn!("Audio: {e}"),
    }

    // MIDI
    let midi_state = std::sync::Arc::new(Mutex::new(MidiState::default()));
    let mut midi_manager = MidiManager::new(midi_state).ok().map(|mut m| {
        let devs = m.refresh_devices();
        shared_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .midi_available_devices = devs;

        // Restore saved device and mappings from config (loaded into EngineState
        // by AppSettings::apply_to_state before this point).
        let (saved_device, saved_mappings) = {
            let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (s.midi_selected_device.clone(), s.midi_mappings.clone())
        };
        if let Some(ref device) = saved_device {
            match m.connect(device) {
                Ok(()) => log::info!("MIDI: restored connection to '{}'", device),
                Err(e) => log::warn!("MIDI: could not restore connection to '{}': {}", device, e),
            }
        }
        if !saved_mappings.is_empty() {
            if let Ok(mut midi_st) = m.state().lock() {
                midi_st.mappings = saved_mappings
                    .iter()
                    .map(|s| {
                        rustjay_control::MidiMapping::new(
                            s.kind,
                            s.selector,
                            s.channel,
                            &s.name,
                            &s.param_path,
                            s.min_value,
                            s.max_value,
                        )
                    })
                    .collect();
                log::info!("MIDI: restored {} mapping(s)", midi_st.mappings.len());
            }
        }
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
        host: web_host.clone(),
        port: web_port,
        app_name: app_name.clone(),
        enabled: false,
        lan_trust: web_lan,
        token: None,
    });
    web_server.register_default_parameters();
    web_server.register_parameters(&descriptors);
    shared_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .web_app_name = app_name.clone();

    // Pre-populate web-server caches before accepting connections so that
    // every new WebSocket gets structural state immediately on connect.
    {
        let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        web_server.send_control_state(&rustjay_control::ControlStateJson {
            osc_enabled: s.osc_enabled,
            osc_port: s.osc_port,
            midi_enabled: s.midi_enabled,
            midi_selected_device: s.midi_selected_device.clone(),
            midi_devices: s.midi_available_devices.clone(),
            midi_mappings: s.midi_mappings.clone(),
            midi_learn_active: s.midi_learn_active,
            midi_learning_param_name: s.midi_learning_param_name.clone(),
        });
        let mod_eng = s.modulation.lock().unwrap_or_else(|e| e.into_inner());
        web_server.send_modulation_state(&rustjay_control::ModulationStateJson {
            lfos: mod_eng.to_lfo_vec(),
            audio_routes: s.audio_routing.matrix.routes().to_vec(),
            audio_routing_enabled: s.audio_routing.enabled,
            bpm: s.audio.bpm,
            tap_tempo_info: s.audio.tap_tempo_info.clone(),
        });
        let mut input_state =
            gles2
                .get_input_state()
                .unwrap_or_else(|| rustjay_control::InputStateJson {
                    devices: vec![],
                    active_index: None,
                    active_name: String::new(),
                    width: 0,
                    height: 0,
                    fps: 0.0,
                });
        input_state.devices = s.input.available_devices.clone();
        web_server.send_input_state(&input_state);
        log::info!("Web server state caches pre-populated");
    }

    if let Err(e) = web_server.start() {
        log::error!("Web UI failed to start: {e}");
    } else {
        log::info!("Web UI running at http://{}:{}", web_host, web_port);
        // Force initial broadcast so panels populate immediately on first connect.
        web_server.control_dirty = true;
        web_server.modulation_dirty = true;
        web_server.input_dirty = true;
    }

    // Presets — keep bank alive for the entire loop (WR-9.3)
    let mut preset_bank: Option<PresetBank> = presets_dir_for(&app_name).ok().map(|dir| {
        let bank = PresetBank::new(dir);
        let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
        let slots: [Option<String>; 8] =
            std::array::from_fn(|i| bank.get_slot_name(i + 1).map(|s| s.to_string()));
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        state.preset_names = names;
        state.preset_quick_slot_names = slots;
        bank
    });

    // Track last-broadcast MIDI mapping snapshot for change detection (WR-3.3 / WR-6)
    let mut last_broadcast_mappings: Vec<rustjay_core::MidiMappingSnapshot> = Vec::new();

    // DRM/GBM/EGL context
    let (mut gles2_state, w, h) = try_create_drm_gles2_context("/dev/dri/card0")?;
    {
        let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        gles2.init_gl(&gles2_state.gl, w, h, &state)?;
    }
    log::info!("DRM render loop starting at {w}×{h}");

    // Main render loop
    let mut last_frame_start = std::time::Instant::now();
    let mut elapsed = 0.0f32;
    loop {
        let frame_start = std::time::Instant::now();
        let delta_time = frame_start
            .duration_since(last_frame_start)
            .as_secs_f32()
            .min(0.1);
        last_frame_start = frame_start;
        elapsed += delta_time;

        // ── Poll engine services ──────────────────────────────────────────────
        // Audio: push latest FFT + volume into shared state
        {
            let fft = analyzer.get_fft();
            let volume = analyzer.get_volume();
            let beat = analyzer.is_beat();
            let phase = analyzer.get_beat_phase();
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            analyzer.set_amplitude(state.audio.amplitude);
            analyzer.set_smoothing(state.audio.smoothing);
            if state.audio.enabled {
                state.audio.fft = fft;
                state.audio.volume = volume;
                state.audio.beat = beat;
                state.audio.beat_phase = phase;
                state.reset_custom_params_to_base();
            }
        }

        // Apply dirty MIDI mapping values to parameters (desktop parity)
        if let Some(ref manager) = midi_manager {
            let dirty: Vec<(String, f32)> = {
                if let Ok(mut midi_state) = manager.state().lock() {
                    midi_state
                        .mappings
                        .iter_mut()
                        .filter(|m| m.is_dirty())
                        .map(|m| (m.param_path.clone(), m.get_scaled_value()))
                        .collect()
                } else {
                    vec![]
                }
            };
            if let Ok(mut state) = shared_state.lock() {
                for (path, value) in dirty {
                    match path.as_str() {
                        "color/hue_shift" => {
                            state.hsb_params.hue_shift = value.clamp(-180.0, 180.0);
                            state.hsb_param_bases.hue_shift = state.hsb_params.hue_shift;
                        }
                        "color/saturation" => {
                            state.hsb_params.saturation = value.clamp(0.0, 2.0);
                            state.hsb_param_bases.saturation = state.hsb_params.saturation;
                        }
                        "color/brightness" => {
                            state.hsb_params.brightness = value.clamp(0.0, 2.0);
                            state.hsb_param_bases.brightness = state.hsb_params.brightness;
                        }
                        "audio/amplitude" => {
                            state.audio.amplitude = value.clamp(0.0, 5.0);
                        }
                        "audio/smoothing" => {
                            state.audio.smoothing = value.clamp(0.0, 1.0);
                        }
                        _ => {
                            if let Some(id) = path.split('/').last() {
                                if state.param_descriptors.iter().any(|d| d.id == id) {
                                    state.set_param_base(id, value);
                                }
                            }
                        }
                    }
                }
                let (h, s, b) = (
                    state.hsb_params.hue_shift,
                    state.hsb_params.saturation,
                    state.hsb_params.brightness,
                );
                state.audio_routing.update_base_values(h, s, b);
            }
        }

        // Update LFO phases and apply modulations to params (post-MIDI so LFO adds on top)
        {
            let (mod_arc, bpm, stable_beat_phase, fft_snapshot, volume) = {
                let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let mod_arc = state.modulation.clone();
                let bpm = state.effective_bpm().max(1.0);
                let stable_beat_phase = state.stable_beat_phase();
                let volume = state.audio.volume;
                let fft: Vec<f32> = if state.audio.enabled {
                    state.audio.fft.clone()
                } else {
                    Vec::new()
                };
                (mod_arc, bpm, stable_beat_phase, fft, volume)
            };
            let audio = {
                let mut values = rustjay_core::modulation::AudioValues::default();
                if !fft_snapshot.is_empty() {
                    values.sources.insert(
                        0,
                        rustjay_core::modulation::AudioSourceValues {
                            fft: &fft_snapshot,
                            level: volume,
                            sample_rate: 48000.0,
                        },
                    );
                }
                values
            };

            let offsets = {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                mod_eng.update(elapsed, bpm, stable_beat_phase, &audio);
                let mut offsets = Vec::with_capacity(mod_eng.assignments.len());
                for param_id in mod_eng.assignments.keys() {
                    let offset = mod_eng.get_modulation(param_id);
                    offsets.push((param_id.clone(), offset));
                }
                offsets
            };

            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.modulation_offsets = offsets;

            // HSB params are read modulated on demand via get_param();
            // pre-computing them here would double-modulate (F4).
            let (h, s, b) = (
                state.hsb_params.hue_shift,
                state.hsb_params.saturation,
                state.hsb_params.brightness,
            );
            state.audio_routing.update_base_values(h, s, b);
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
        let mut preset_dirty = false;
        {
            while let Ok(cmd) = web_server.command_rx.try_recv() {
                match cmd {
                    WebServerCommand::Set { id, value } => {
                        if let Ok(mut state) = shared_state.lock() {
                            match id.as_str() {
                                "color/hue_shift" => {
                                    state.hsb_params.hue_shift = value.clamp(-180.0, 180.0);
                                    state.hsb_param_bases.hue_shift = state.hsb_params.hue_shift;
                                    let (h, s, b) = (
                                        state.hsb_params.hue_shift,
                                        state.hsb_params.saturation,
                                        state.hsb_params.brightness,
                                    );
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/saturation" => {
                                    state.hsb_params.saturation = value.clamp(0.0, 2.0);
                                    state.hsb_param_bases.saturation = state.hsb_params.saturation;
                                    let (h, s, b) = (
                                        state.hsb_params.hue_shift,
                                        state.hsb_params.saturation,
                                        state.hsb_params.brightness,
                                    );
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/brightness" => {
                                    state.hsb_params.brightness = value.clamp(0.0, 2.0);
                                    state.hsb_param_bases.brightness = state.hsb_params.brightness;
                                    let (h, s, b) = (
                                        state.hsb_params.hue_shift,
                                        state.hsb_params.saturation,
                                        state.hsb_params.brightness,
                                    );
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
                                        format!("{}/{}", d.category.name().to_lowercase(), d.id)
                                            == id
                                    }) {
                                        let (desc_id, desc_min, desc_max) =
                                            (desc.id.clone(), desc.min, desc.max);
                                        state.set_param_base(
                                            &desc_id,
                                            value.clamp(desc_min, desc_max),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    WebServerCommand::Input(cmd) => {
                        // Do NOT hold shared_state lock here — open_webcam can block.
                        let gl = gles2_state.gl.clone();
                        gles2.handle_input_command(&gl, cmd);
                        web_server.input_dirty = true;
                    }
                    WebServerCommand::Control(ctrl) => match ctrl {
                        rustjay_control::ControlWebCommand::Osc { enabled: true } => {
                            if let Err(e) = osc_server.start() {
                                log::error!("Failed to start OSC server: {}", e);
                            } else {
                                shared_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .osc_enabled = true;
                                web_server.control_dirty = true;
                            }
                        }
                        rustjay_control::ControlWebCommand::Osc { enabled: false } => {
                            osc_server.stop();
                            shared_state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .osc_enabled = false;
                            web_server.control_dirty = true;
                        }
                        rustjay_control::ControlWebCommand::OscSetPort { port } => {
                            osc_server.stop();
                            let host = shared_state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .osc_host
                                .clone();
                            let new_server = OscServer::new(&host, port, "/rustjay");
                            if let Ok(mut state) = new_server.state().lock() {
                                state.register_default_parameters();
                            }
                            osc_server = new_server;
                            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.osc_port = port;
                            state.osc_enabled = false;
                            web_server.control_dirty = true;
                        }
                        rustjay_control::ControlWebCommand::MidiLearn { param_id } => {
                            let (name, min, max) = {
                                let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state
                                    .param_descriptors
                                    .iter()
                                    .find(|d| {
                                        let full = format!(
                                            "{}/{}",
                                            d.category.name().to_lowercase(),
                                            d.id
                                        );
                                        full == param_id || d.id == param_id
                                    })
                                    .map(|d| (d.name.clone(), d.min, d.max))
                                    .unwrap_or_default()
                            };
                            if !name.is_empty() {
                                if let Some(ref mut m) = midi_manager {
                                    m.start_learn(&param_id, &name, min, max);
                                    shared_state
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .midi_learn_active = true;
                                    web_server.control_dirty = true;
                                }
                            } else {
                                log::warn!("MidiLearn: unknown param_id '{}'", param_id);
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiLearnCancel => {
                            if let Some(ref mut m) = midi_manager {
                                m.cancel_learn();
                                shared_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .midi_learn_active = false;
                                web_server.control_dirty = true;
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiUnlearn { cc, channel } => {
                            if let Some(ref m) = midi_manager {
                                if let Ok(mut midi_st) = m.state().lock() {
                                    midi_st.mappings.retain(|mapping| {
                                        !(mapping.selector == cc && mapping.channel == channel)
                                    });
                                    web_server.control_dirty = true;
                                }
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiRefreshDevices => {
                            if let Some(ref mut m) = midi_manager {
                                let devs = m.refresh_devices();
                                shared_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .midi_available_devices = devs;
                                web_server.control_dirty = true;
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiSelectDevice { device } => {
                            if let Some(ref mut m) = midi_manager {
                                match m.connect(&device) {
                                    Ok(()) => {
                                        shared_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .midi_selected_device = Some(device.clone());
                                        web_server.control_dirty = true;
                                    }
                                    Err(e) => log::error!("MIDI connect failed: {}", e),
                                }
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiDisconnect => {
                            if let Some(ref mut m) = midi_manager {
                                m.disconnect();
                                shared_state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .midi_selected_device = None;
                                web_server.control_dirty = true;
                            }
                        }
                    },
                    WebServerCommand::Modulation(mod_cmd) => {
                        match mod_cmd {
                            rustjay_control::ModulationWebCommand::LfoSet { slot, config } => {
                                let uuid = format!("lfo_{slot}");
                                let mut state =
                                    shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                let mut mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(idx) = mod_eng.sources.iter().position(|s| s.uuid == uuid) {
                                    if let rustjay_core::modulation::ModulationSource::LFO { phase, last_beat_phase, .. } = &mod_eng.sources[idx].source {
                                        let (existing_phase, existing_last_beat) = (*phase, *last_beat_phase);
                                        let waveform = match config.waveform {
                                            rustjay_core::lfo::Waveform::Sine => rustjay_core::modulation::LFOWaveform::Sine,
                                            rustjay_core::lfo::Waveform::Triangle => rustjay_core::modulation::LFOWaveform::Triangle,
                                            rustjay_core::lfo::Waveform::Square => rustjay_core::modulation::LFOWaveform::Square,
                                            rustjay_core::lfo::Waveform::Ramp | rustjay_core::lfo::Waveform::Saw => rustjay_core::modulation::LFOWaveform::Sawtooth,
                                        };
                                        mod_eng.sources[idx].source = rustjay_core::modulation::ModulationSource::LFO {
                                            waveform,
                                            frequency: config.rate,
                                            phase: existing_phase,
                                            amplitude: config.amplitude,
                                            bipolar: true,
                                            tempo_sync: config.tempo_sync,
                                            division: config.division,
                                            phase_offset_degrees: config.phase_offset,
                                            enabled: config.enabled,
                                            last_beat_phase: existing_last_beat,
                                        };
                                    }
                                }
                                web_server.modulation_dirty = true;
                            }
                            rustjay_control::ModulationWebCommand::LfoEnable { slot, enabled } => {
                                let uuid = format!("lfo_{slot}");
                                let mut state =
                                    shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                let mut mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(idx) = mod_eng.sources.iter().position(|s| s.uuid == uuid) {
                                    if let rustjay_core::modulation::ModulationSource::LFO { ref mut enabled: e, .. } = mod_eng.sources[idx].source {
                                        *e = enabled;
                                    }
                                }
                                web_server.modulation_dirty = true;
                            }
                            rustjay_control::ModulationWebCommand::AudioRoute {
                                param_id,
                                band,
                                depth,
                            } => {
                                let target = param_id_to_modulation_target(&param_id);
                                let mut state =
                                    shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                let ids_to_remove: Vec<usize> = state
                                    .audio_routing
                                    .matrix
                                    .routes()
                                    .iter()
                                    .filter(|r| r.band == band && r.target == target)
                                    .map(|r| r.id)
                                    .collect();
                                for id in ids_to_remove {
                                    state.audio_routing.matrix.remove_route(id);
                                }
                                if let Some(id) = state.audio_routing.matrix.add_route(band, target)
                                {
                                    if let Some(route) =
                                        state.audio_routing.matrix.get_route_mut(id)
                                    {
                                        route.amount = depth;
                                    }
                                }
                                web_server.modulation_dirty = true;
                            }
                            rustjay_control::ModulationWebCommand::AudioUnroute { param_id } => {
                                let target = param_id_to_modulation_target(&param_id);
                                let mut state =
                                    shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                let ids_to_remove: Vec<usize> = state
                                    .audio_routing
                                    .matrix
                                    .routes()
                                    .iter()
                                    .filter(|r| r.target == target)
                                    .map(|r| r.id)
                                    .collect();
                                for id in ids_to_remove {
                                    state.audio_routing.matrix.remove_route(id);
                                }
                                web_server.modulation_dirty = true;
                            }
                            rustjay_control::ModulationWebCommand::TapTempo => {
                                use std::time::{SystemTime, UNIX_EPOCH};
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs_f64();
                                {
                                    let mut state =
                                        shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    let is_first_tap = now - state.audio.last_tap_time > 2.0;
                                    if is_first_tap {
                                        state.audio.tap_times.clear();
                                        let mut mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
                                        for entry in mod_eng.sources.iter_mut() {
                                            if let rustjay_core::modulation::ModulationSource::LFO { phase, last_beat_phase, .. } = &mut entry.source {
                                                *phase = 0.0;
                                                *last_beat_phase = 0.0;
                                            }
                                        }
                                    }
                                    state.audio.tap_times.push(now);
                                    state.audio.last_tap_time = now;
                                    if state.audio.tap_times.len() > 8 {
                                        state.audio.tap_times.remove(0);
                                    }
                                    state.audio.beat_phase = 0.0;
                                    // 2 taps gives one interval — enough for an immediate BPM estimate.
                                    if state.audio.tap_times.len() >= 2 {
                                        let n = state.audio.tap_times.len();
                                        let mut intervals = Vec::new();
                                        for i in 1..n {
                                            intervals.push(
                                                state.audio.tap_times[i]
                                                    - state.audio.tap_times[i - 1],
                                            );
                                        }
                                        let avg_interval: f64 =
                                            intervals.iter().sum::<f64>() / intervals.len() as f64;
                                        if avg_interval > 0.1 && avg_interval < 3.0 {
                                            state.audio.bpm = (60.0 / avg_interval) as f32;
                                            state.audio.tap_tempo_info =
                                                format!("{:.1} BPM ({} taps)", state.audio.bpm, n);
                                        }
                                    } else {
                                        state.audio.tap_tempo_info = "Tap again…".to_string();
                                    }
                                }
                                web_server.modulation_dirty = true;
                            }
                        }
                    }
                    WebServerCommand::Preset(preset_cmd) => match preset_cmd {
                        rustjay_control::PresetWebCommand::List => {
                            preset_dirty = true;
                        }
                        rustjay_control::PresetWebCommand::Save { name } => {
                            let valid = !name.is_empty()
                                && name.len() <= 64
                                && !name.contains('/')
                                && !name.contains('\\')
                                && !name.contains("..");
                            if valid {
                                if let Some(ref mut bank) = preset_bank {
                                    let preset = {
                                        let state =
                                            shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                        rustjay_presets::Preset::from_state(&name, &state)
                                    };
                                    match bank.add_preset(preset) {
                                        Ok(_) => {
                                            let mut state = shared_state
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            state.preset_names = bank
                                                .presets
                                                .iter()
                                                .map(|p| p.name.clone())
                                                .collect();
                                            preset_dirty = true;
                                        }
                                        Err(e) => log::error!("Preset save failed: {e}"),
                                    }
                                }
                            } else {
                                log::warn!("Web preset save: invalid name '{}'", name);
                            }
                        }
                        rustjay_control::PresetWebCommand::Load { index } => {
                            if let Some(ref mut bank) = preset_bank {
                                let mut state =
                                    shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                if let Err(e) = bank.apply_preset(index, &mut state) {
                                    log::error!("Preset load failed: {e}");
                                } else {
                                    preset_dirty = true;
                                }
                            }
                        }
                        rustjay_control::PresetWebCommand::Delete { index } => {
                            if let Some(ref mut bank) = preset_bank {
                                if let Err(e) = bank.delete_preset(index) {
                                    log::error!("Preset delete failed: {e}");
                                } else {
                                    let mut state =
                                        shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.preset_names =
                                        bank.presets.iter().map(|p| p.name.clone()).collect();
                                    preset_dirty = true;
                                }
                            }
                        }
                    },
                }
            }
        }

        // Poll async device enumeration result from the Tokio thread (WR-2.1)
        let devices = {
            if let Ok(mut ws) = web_server.state.lock() {
                if let Ok(mut pd) = ws.pending_devices.lock() {
                    pd.take()
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(devices) = devices {
            if let Ok(mut state) = shared_state.lock() {
                state.input.available_devices = devices;
                web_server.input_dirty = true;
            }
        }

        // MIDI mapping change detection (WR-3.3 / WR-6)
        if let Some(ref m) = midi_manager {
            if let Ok(midi_st) = m.state().lock() {
                let current: Vec<rustjay_core::MidiMappingSnapshot> = midi_st
                    .mappings
                    .iter()
                    .map(|m| rustjay_core::MidiMappingSnapshot {
                        name: m.name.clone(),
                        param_path: m.param_path.clone(),
                        kind: m.kind,
                        selector: m.selector,
                        channel: m.channel,
                        min_value: m.min_value,
                        max_value: m.max_value,
                    })
                    .collect();
                if current != last_broadcast_mappings {
                    // Write back to EngineState so AppSettings::from_state() persists them.
                    if let Ok(mut state) = shared_state.lock() {
                        state.midi_mappings = current.clone();
                        state.save_settings_requested = true;
                    }
                    last_broadcast_mappings = current;
                    web_server.control_dirty = true;
                }
            }
        }

        let target_fps = shared_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .target_fps;

        // Web: broadcast current parameter values to connected clients
        if web_server.is_running() {
            if let Ok(state) = shared_state.lock() {
                web_server.update_parameter("color/hue_shift", state.hsb_params.hue_shift);
                web_server.update_parameter("color/saturation", state.hsb_params.saturation);
                web_server.update_parameter("color/brightness", state.hsb_params.brightness);
                web_server
                    .update_parameter("color/enabled", if state.color_enabled { 1.0 } else { 0.0 });
                web_server.update_parameter("audio/amplitude", state.audio.amplitude);
                web_server.update_parameter("audio/smoothing", state.audio.smoothing);
                web_server
                    .update_parameter("audio/enabled", if state.audio.enabled { 1.0 } else { 0.0 });
                web_server.update_parameter(
                    "audio/normalize",
                    if state.audio.normalize { 1.0 } else { 0.0 },
                );
                web_server.update_parameter(
                    "audio/pink_noise",
                    if state.audio.pink_noise_shaping {
                        1.0
                    } else {
                        0.0
                    },
                );
                web_server.update_parameter(
                    "output/fullscreen",
                    if state.output_fullscreen { 1.0 } else { 0.0 },
                );
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

        // Drain structural dirty flags — broadcast control/preset/input/modulation state to web panels.
        if web_server.is_running() {
            if web_server.input_dirty {
                let mut input_state =
                    gles2
                        .get_input_state()
                        .unwrap_or_else(|| rustjay_control::InputStateJson {
                            devices: vec![],
                            active_index: None,
                            active_name: String::new(),
                            width: 0,
                            height: 0,
                            fps: 0.0,
                        });
                if let Ok(state) = shared_state.lock() {
                    input_state.devices = state.input.available_devices.clone();
                }
                web_server.send_input_state(&input_state);
                web_server.input_dirty = false;
            }
            if web_server.control_dirty {
                let (
                    osc_enabled,
                    osc_port,
                    midi_enabled,
                    midi_selected_device,
                    midi_devices,
                    midi_learn_active,
                    midi_learning_param_name,
                ) = {
                    let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        s.osc_enabled,
                        s.osc_port,
                        s.midi_enabled,
                        s.midi_selected_device.clone(),
                        s.midi_available_devices.clone(),
                        s.midi_learn_active,
                        s.midi_learning_param_name.clone(),
                    )
                };
                let midi_mappings: Vec<rustjay_core::MidiMappingSnapshot> =
                    if let Some(ref m) = midi_manager {
                        if let Ok(midi_st) = m.state().lock() {
                            midi_st
                                .mappings
                                .iter()
                                .map(|m| rustjay_core::MidiMappingSnapshot {
                                    name: m.name.clone(),
                                    param_path: m.param_path.clone(),
                                    kind: m.kind,
                                    selector: m.selector,
                                    channel: m.channel,
                                    min_value: m.min_value,
                                    max_value: m.max_value,
                                })
                                .collect()
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                web_server.send_control_state(&rustjay_control::ControlStateJson {
                    osc_enabled,
                    osc_port,
                    midi_enabled,
                    midi_selected_device,
                    midi_devices,
                    midi_mappings,
                    midi_learn_active,
                    midi_learning_param_name,
                });
                web_server.control_dirty = false;
            }
            if web_server.modulation_dirty {
                if let Ok(state) = shared_state.lock() {
                    let mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
                    web_server.send_modulation_state(&rustjay_control::ModulationStateJson {
                        lfos: mod_eng.to_lfo_vec(),
                        audio_routes: state.audio_routing.matrix.routes().to_vec(),
                        audio_routing_enabled: state.audio_routing.enabled,
                        bpm: state.audio.bpm,
                        tap_tempo_info: state.audio.tap_tempo_info.clone(),
                    });
                }
                web_server.modulation_dirty = false;
            }
            if preset_dirty {
                if let Some(ref bank) = preset_bank {
                    web_server.send_preset_state(&rustjay_control::PresetStateJson {
                        presets: bank
                            .presets
                            .iter()
                            .enumerate()
                            .map(|(i, p)| rustjay_control::PresetInfo {
                                index: i,
                                name: p.name.clone(),
                            })
                            .collect(),
                    });
                }
                preset_dirty = false;
            }
        }

        // Render frame
        let gl = gles2_state.gl.clone();
        let keep = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            match gles2.render_frame(&gl, &state) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("DRM render: {e}");
                    true
                }
            }
        };
        if !keep {
            break;
        }

        // Page flip (provides vsync)
        if let Err(e) = gles2_state.present() {
            log::error!("DRM present: {e}");
        }

        // Settings persist
        let should_save = {
            let mut s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            if s.save_settings_requested {
                s.save_settings_requested = false;
                true
            } else {
                false
            }
        };
        if should_save {
            let settings =
                AppSettings::from_state(&shared_state.lock().unwrap_or_else(|e| e.into_inner()));
            if let Err(e) = settings.save(&app_name) {
                log::error!("Save: {e}");
            }
        }

        // Extra sleep only when page flip returns faster than the target rate
        let target_dur = std::time::Duration::from_micros(1_000_000 / target_fps.max(1) as u64);
        let elapsed = frame_start.elapsed();
        if elapsed < target_dur {
            std::thread::sleep(target_dur - elapsed);
        }
    }

    let settings = AppSettings::from_state(&shared_state.lock().unwrap_or_else(|e| e.into_inner()));
    let _ = settings.save(&app_name);
    analyzer.stop();
    osc_server.stop();
    log::info!("DRM shutdown complete");
    Ok(())
}
