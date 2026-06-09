use super::{App, WindowAction};
use rustjay_core::EffectPlugin;
use rustjay_gui::{ControlGui, ImGuiRenderer};
#[cfg(feature = "egui")]
use rustjay_gui::{EguiControlGui, EguiRenderer};
use rustjay_render::WgpuEngine;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowAttributes;

/// Minimum interval between control-window UI rebuilds (~30 Hz). Independent of
/// the output `target_fps`: the output keeps rendering at full rate, only the
/// imgui/egui control window is throttled to cut per-frame buffer allocations.
const UI_RENDER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

impl<P: EffectPlugin> ApplicationHandler<WindowAction> for App<P> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // ── DRM/GBM path: bypass winit window + wgpu entirely ────────────────
        #[cfg(feature = "drm-gles2")]
        if self.drm_gles2 && self.gles2_effect.is_some() && self.gles2_state.is_none() {
            match crate::gles2::try_create_drm_gles2_context("/dev/dri/card0") {
                Ok((mut gles2_state, w, h)) => {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(effect) = self.gles2_effect.as_mut() {
                        if let Err(e) = effect.init_gl(&gles2_state.gl.clone(), w, h, &state) {
                            log::error!("DRM GLES 2.0 effect init failed: {e}");
                            event_loop.exit();
                            return;
                        }
                    }
                    drop(state);
                    self.gles2_state = Some(gles2_state);
                    log::info!("DRM/GBM GLES 2.0 render path active — no compositor required");
                }
                Err(e) => {
                    log::error!("Failed to create DRM/GBM context: {e}");
                    event_loop.exit();
                }
            }
            return; // Skip window + wgpu init
        }

        if self.wgpu_instance.is_none() {
            #[cfg(target_os = "macos")]
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });

            // On Linux, wgpu always passes None as the display handle to wgpu-hal, so the
            // GLES backend falls back to a surfaceless EGL display that has no EGL_WINDOW_BIT
            // configs. We bypass this by initialising the GLES HAL instance directly with
            // the event loop's display handle (Wayland or X11), then wrapping it with
            // wgpu::Instance::from_hal. Vulkan (Pi 4/5) is unaffected — it doesn't use EGL.
            #[cfg(target_os = "linux")]
            let instance = {
                use raw_window_handle::HasDisplayHandle as _;
                use wgpu::hal::{api::Gles, Api as _, Instance as HalInstance};

                // wgpu::Instance::new() always passes None as the display handle to
                // wgpu-hal, causing EGL to use the surfaceless platform (no EGL_WINDOW_BIT
                // configs). We bypass this by initialising the GLES HAL instance directly
                // with the event loop's display handle (Wayland or X11).
                let display_handle = event_loop.display_handle().ok();

                let hal_desc = wgpu::hal::InstanceDescriptor {
                    name: "rustjay-gles",
                    flags: wgpu::InstanceFlags::from_build_config(),
                    memory_budget_thresholds: Default::default(),
                    backend_options: Default::default(),
                    telemetry: None,
                    display: display_handle,
                };

                match unsafe { <Gles as wgpu::hal::Api>::Instance::init(&hal_desc) } {
                    Ok(gles) => unsafe { wgpu::Instance::from_hal::<Gles>(gles) },
                    Err(e) => {
                        log::warn!("GLES HAL init failed ({e}), falling back to default backends");
                        wgpu::Instance::new(wgpu::InstanceDescriptor {
                            backends: wgpu::Backends::all(),
                            ..wgpu::InstanceDescriptor::new_without_display_handle()
                        })
                    }
                }
            };

            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });

            self.wgpu_instance = Some(instance);
        }
        let Some(instance) = self.wgpu_instance.as_ref() else {
            return;
        };

        let no_primary = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.no_primary_output
        };

        if self.output_window.is_none() {
            let (output_width, output_height, fullscreen) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.output_width,
                    state.output_height,
                    state.output_fullscreen,
                )
            };

            let window_attrs = WindowAttributes::default()
                .with_title("RustJay Output")
                .with_inner_size(winit::dpi::LogicalSize::new(output_width, output_height))
                .with_resizable(true)
                .with_decorations(true)
                .with_visible(!no_primary);

            let window = match event_loop.create_window(window_attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    log::error!("Failed to create output window: {}", e);
                    event_loop.exit();
                    return;
                }
            };

            if fullscreen {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
            if !no_primary {
                window.set_cursor_visible(false);
            }
            self.output_window = Some(Arc::clone(&window));
            if no_primary {
                self.output_occluded = true;
            }

            // ── GLES 2.0 path (Pi 2 / hardware without GLES 3.0 UBOs) ──────────
            #[cfg(feature = "gles2")]
            if self.gles2_effect.is_some() {
                use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
                use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

                let (wayland_display, wayland_surface) = {
                    let dh = event_loop.display_handle().ok().map(|h| h.as_raw());
                    let wh = window.window_handle().ok().map(|h| h.as_raw());
                    match (dh, wh) {
                        (Some(RawDisplayHandle::Wayland(d)), Some(RawWindowHandle::Wayland(w))) => {
                            (d.display.as_ptr(), w.surface.as_ptr())
                        }
                        _ => {
                            log::error!("GLES 2.0 path requires a Wayland display/surface");
                            event_loop.exit();
                            return;
                        }
                    }
                };

                let size = window.inner_size();
                match crate::gles2::try_create_gles2_context(
                    wayland_display as *mut _,
                    wayland_surface as *mut _,
                    size.width,
                    size.height,
                ) {
                    Ok(mut gles2_state) => {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(effect) = self.gles2_effect.as_mut() {
                            if let Err(e) = effect.init_gl(
                                &gles2_state.gl,
                                gles2_state.width,
                                gles2_state.height,
                                &state,
                            ) {
                                log::error!("GLES 2.0 effect init failed: {e}");
                                event_loop.exit();
                                return;
                            }
                        }
                        drop(state);
                        self.gles2_state = Some(gles2_state);
                        log::info!("GLES 2.0 render path active");
                    }
                    Err(e) => {
                        log::error!("Failed to create GLES 2.0 context: {e}");
                        event_loop.exit();
                    }
                }
                return; // skip wgpu engine init
            }

            // ── wgpu path ────────────────────────────────────────────────────
            let shared_state = Arc::clone(&self.shared_state);
            let plugin = match self.plugin.take() {
                Some(p) => p,
                None => {
                    log::error!("Plugin already consumed — resumed() called twice?");
                    return;
                }
            };
            match pollster::block_on(WgpuEngine::new(instance, window, shared_state, plugin)) {
                Ok(engine) => {
                    log::info!("Output engine initialized");
                    self.wgpu_adapter = Some(engine.adapter.clone());
                    self.wgpu_device = Some(Arc::clone(&engine.device));
                    self.wgpu_queue = Some(Arc::clone(&engine.queue));
                    self.output_engine = Some(engine);

                    if let (Some(ref mut manager), Some(device), Some(queue)) = (
                        self.input_manager.as_mut(),
                        self.wgpu_device.as_ref(),
                        self.wgpu_queue.as_ref(),
                    ) {
                        manager.initialize(device, queue);
                        log::info!("InputManager initialized with GPU resources");

                        // Start the saved webcam device synchronously here rather than
                        // deferring to the frame loop.  On slow hardware (Pi 2 llvmpipe)
                        // the first engine.render() can block for many seconds, so
                        // queuing a two-step RefreshDevices → StartWebcam sequence would
                        // never dispatch before the user's process is killed.
                        let idx = self
                            .shared_state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .startup_webcam_device
                            .take();
                        if let Some(idx) = idx {
                            #[cfg(feature = "webcam")]
                            match manager.start_webcam(idx, 1280, 720, 30) {
                                Ok(()) => {
                                    let mut state =
                                        self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.input.is_active = true;
                                    state.input.input_type = rustjay_core::InputType::Webcam;
                                    state.input.source_name = format!("Webcam {}", idx);
                                    state.input.device_index = Some(idx);
                                }
                                Err(e) => log::error!(
                                    "startup_webcam_device: failed to start webcam {}: {:?}",
                                    idx,
                                    e
                                ),
                            }
                        }
                    }
                    if let (Some(ref mut manager), Some(device), Some(queue)) = (
                        self.second_input_manager.as_mut(),
                        self.wgpu_device.as_ref(),
                        self.wgpu_queue.as_ref(),
                    ) {
                        manager.initialize(device, queue);
                        log::info!("Second InputManager initialized with GPU resources");
                    }
                }
                Err(err) => {
                    log::error!("Failed to create output engine: {}", err);
                    event_loop.exit();
                    return;
                }
            }

            #[cfg(feature = "projection")]
            {
                let inst = self.wgpu_instance.as_ref();
                let device = self.wgpu_device.as_ref();
                let queue = self.wgpu_queue.as_ref();
                let adapter = self.wgpu_adapter.as_ref();
                if let (Some(sub), Some(inst), Some(device), Some(queue), Some(adapter)) =
                    (self.projection_subsystem.as_ref(), inst, device, queue, adapter)
                {
                    let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
                    sub.create_pending(event_loop, inst, Arc::clone(device), Arc::clone(queue), adapter);
                }
            }
        }

        if !self.nogui && self.control_window.is_none() {
            if let Some(ref engine) = self.output_engine {
                let device = Arc::clone(&engine.device);
                let queue = Arc::clone(&engine.queue);

                let window_attrs = WindowAttributes::default()
                    .with_title("RustJay - Control")
                    .with_inner_size(winit::dpi::LogicalSize::new(1200u32, 800u32))
                    .with_resizable(true)
                    .with_decorations(true);

                let window = match event_loop.create_window(window_attrs) {
                    Ok(w) => Arc::new(w),
                    Err(e) => {
                        log::error!("Failed to create control window: {}", e);
                        return;
                    }
                };
                self.control_window = Some(Arc::clone(&window));

                let adapter = match self.wgpu_adapter.as_ref() {
                    Some(a) => a,
                    None => {
                        log::error!("wgpu adapter not initialized before control window");
                        return;
                    }
                };

                let scale_factor = window.scale_factor();
                if self.use_egui {
                    #[cfg(feature = "egui")]
                    match pollster::block_on(EguiRenderer::new(
                        instance,
                        adapter,
                        device,
                        queue,
                        window,
                        scale_factor,
                    )) {
                        Ok(mut renderer) => {
                            match EguiControlGui::new(Arc::clone(&self.shared_state)) {
                                Ok(mut gui) => {
                                    let input_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    let second_input_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    let output_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    gui.set_input_preview_texture(input_preview_id);
                                    gui.set_second_input_preview_texture(second_input_preview_id);
                                    gui.set_output_preview_texture(output_preview_id);
                                    log::info!("Created egui preview textures");

                                    // Move custom tabs into the GUI
                                    gui.custom_tabs = std::mem::take(&mut self.custom_tabs_egui);

                                    self.egui_control_gui = Some(gui);
                                    self.egui_renderer = Some(renderer);
                                }
                                Err(err) => {
                                    log::error!("Failed to create egui control GUI: {}", err)
                                }
                            }
                        }
                        Err(err) => log::error!("Failed to create egui renderer: {}", err),
                    }
                } else {
                    match pollster::block_on(ImGuiRenderer::new(
                        instance,
                        adapter,
                        device,
                        queue,
                        window,
                        scale_factor,
                    )) {
                        Ok(mut renderer) => {
                            match ControlGui::new(Arc::clone(&self.shared_state)) {
                                Ok(mut gui) => {
                                    let input_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    let second_input_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    let output_preview_id =
                                        renderer.create_preview_texture(1920, 1080);
                                    gui.set_input_preview_texture(input_preview_id);
                                    gui.set_second_input_preview_texture(second_input_preview_id);
                                    gui.set_output_preview_texture(output_preview_id);
                                    log::info!("Created preview textures");

                                    // Move custom tabs into the GUI
                                    gui.custom_tabs = std::mem::take(&mut self.custom_tabs_imgui);

                                    self.control_gui = Some(gui);
                                    self.imgui_renderer = Some(renderer);
                                }
                                Err(err) => log::error!("Failed to create control GUI: {}", err),
                            }
                        }
                        Err(err) => log::error!("Failed to create ImGui renderer: {}", err),
                    }
                }
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowAction) {
        match event {
            WindowAction::RecreateWindows => {
                if let Some(ref window) = self.output_window {
                    window.set_visible(true);
                    self.output_occluded = false;
                    let fullscreen = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.output_fullscreen
                    };
                    if fullscreen {
                        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                    }
                    window.set_cursor_visible(false);
                    log::info!("Output window shown");
                }
                if let Some(ref window) = self.control_window {
                    window.set_visible(true);
                    self.control_visible = true;
                    log::info!("Control window shown");
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if let Some(output_window) = self.output_window.as_ref() {
            if window_id == output_window.id() {
                match event {
                    WindowEvent::CloseRequested => {
                        let window = Arc::clone(output_window);
                        self.save_settings();
                        window.set_visible(false);
                        self.output_occluded = true;
                        log::info!("Output window hidden");
                    }
                    WindowEvent::CursorEntered { .. } => {
                        output_window.set_cursor_visible(false);
                    }
                    WindowEvent::KeyboardInput { ref event, .. } => {
                        if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Shift) =
                            &event.logical_key
                        {
                            self.shift_pressed = event.state == winit::event::ElementState::Pressed;
                        }
                        if event.state == winit::event::ElementState::Pressed {
                            match &event.logical_key {
                                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => {
                                    self.save_settings();
                                    event_loop.exit();
                                }
                                winit::keyboard::Key::Character(ch) => {
                                    let key = ch.to_lowercase();
                                    if self.shift_pressed && key == "f" {
                                        self.toggle_fullscreen();
                                    }
                                    if self.shift_pressed && key == "t" {
                                        self.trigger_tap_tempo();
                                    }
                                }
                                winit::keyboard::Key::Named(named) if self.shift_pressed => {
                                    let slot = match named {
                                        winit::keyboard::NamedKey::F1 => Some(1),
                                        winit::keyboard::NamedKey::F2 => Some(2),
                                        winit::keyboard::NamedKey::F3 => Some(3),
                                        winit::keyboard::NamedKey::F4 => Some(4),
                                        winit::keyboard::NamedKey::F5 => Some(5),
                                        winit::keyboard::NamedKey::F6 => Some(6),
                                        winit::keyboard::NamedKey::F7 => Some(7),
                                        winit::keyboard::NamedKey::F8 => Some(8),
                                        _ => None,
                                    };
                                    if let Some(s) = slot {
                                        if let Some(ref mut bank) = self.preset_bank {
                                            let mut state = self
                                                .shared_state
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            let _ = bank.apply_slot(s, &mut state);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowEvent::Occluded(occluded) => {
                        self.output_occluded = occluded;
                    }
                    WindowEvent::Resized(size) if !self.output_occluded => {
                        if let Some(ref mut engine) = self.output_engine {
                            engine.resize(size.width, size.height);
                        }
                    }
                    _ => {}
                }
                return;
            }
        }

        if let Some(control_window) = self.control_window.as_ref() {
            if window_id == control_window.id() {
                if self.use_egui {
                    #[cfg(feature = "egui")]
                    if let Some(ref mut renderer) = self.egui_renderer {
                        let winit_event = winit::event::Event::WindowEvent {
                            window_id,
                            event: event.clone(),
                        };
                        renderer.handle_event(&winit_event);
                    }
                } else if let Some(ref mut renderer) = self.imgui_renderer {
                    let winit_event = winit::event::Event::WindowEvent {
                        window_id,
                        event: event.clone(),
                    };
                    renderer.handle_event(&winit_event);
                }
                // A control-window event arrived — rebuild the UI next frame
                // immediately rather than waiting out the ~30 Hz throttle, so
                // slider drags and tab clicks stay responsive.
                self.ui_needs_redraw = true;

                match event {
                    WindowEvent::CloseRequested => {
                        let window = Arc::clone(control_window);
                        self.save_settings();
                        window.set_visible(false);
                        self.control_visible = false;
                        log::info!("Control window hidden");
                    }
                    WindowEvent::KeyboardInput { ref event, .. } => {
                        if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Shift) =
                            &event.logical_key
                        {
                            self.shift_pressed = event.state == winit::event::ElementState::Pressed;
                        }
                        if event.state == winit::event::ElementState::Pressed {
                            match &event.logical_key {
                                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => {
                                    self.save_settings();
                                    event_loop.exit();
                                }
                                winit::keyboard::Key::Character(ch) => {
                                    let key = ch.to_lowercase();
                                    if self.shift_pressed && key == "t" {
                                        self.trigger_tap_tempo();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowEvent::Resized(size) if self.control_visible => {
                        if self.use_egui {
                            #[cfg(feature = "egui")]
                            if let Some(ref mut renderer) = self.egui_renderer {
                                renderer.resize(size.width, size.height);
                            }
                        } else if let Some(ref mut renderer) = self.imgui_renderer {
                            renderer.resize(size.width, size.height);
                        }
                    }
                    WindowEvent::ScaleFactorChanged { scale_factor, .. }
                        if self.control_visible =>
                    {
                        if self.use_egui {
                            #[cfg(feature = "egui")]
                            if let Some(ref mut renderer) = self.egui_renderer {
                                renderer.set_scale_factor(scale_factor);
                                let window_size = control_window.inner_size();
                                let logical_width = window_size.width as f32 / scale_factor as f32;
                                let logical_height =
                                    window_size.height as f32 / scale_factor as f32;
                                renderer.set_display_size(logical_width, logical_height);
                            }
                        } else if let Some(ref mut renderer) = self.imgui_renderer {
                            renderer.set_scale_factor(scale_factor);
                            let window_size = control_window.inner_size();
                            let logical_width = window_size.width as f32 / scale_factor as f32;
                            let logical_height = window_size.height as f32 / scale_factor as f32;
                            renderer.set_display_size(logical_width, logical_height);
                        }
                    }
                    _ => {}
                }
            }
        }

        #[cfg(feature = "projection")]
        if let Some(sub) = self.projection_subsystem.as_ref() {
            if let Some(ref device) = self.wgpu_device {
                let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
                sub.handle_window_event(window_id, &event, device, &mut self.shift_pressed);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let frame_start = std::time::Instant::now();
        let now = frame_start;
        self.frame_delta_time = now
            .duration_since(self.last_frame_time)
            .as_secs_f32()
            .clamp(0.001, 0.1);
        self.last_frame_time = now;

        self.dispatch_commands();
        self.poll_device_discovery();
        self.update_input();
        self.update_audio();
        #[cfg(feature = "link")]
        self.update_link();
        #[cfg(feature = "prodj")]
        self.update_prodj();
        self.update_lfo();
        self.update_midi();
        self.update_osc();
        self.update_web();

        // Create any pending projector windows queued at runtime (e.g. from UI).
        #[cfg(feature = "projection")]
        {
            let inst = self.wgpu_instance.as_ref();
            let device = self.wgpu_device.as_ref();
            let queue = self.wgpu_queue.as_ref();
            let adapter = self.wgpu_adapter.as_ref();
            if let (Some(sub), Some(inst), Some(device), Some(queue), Some(adapter)) =
                (self.projection_subsystem.as_ref(), inst, device, queue, adapter)
            {
                let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
                if sub.pending_len() > 0 {
                    sub.create_pending(event_loop, inst, Arc::clone(device), Arc::clone(queue), adapter);
                }
            }
        }

        let should_save = {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            if state.save_settings_requested {
                state.save_settings_requested = false;
                true
            } else {
                false
            }
        };
        if should_save {
            self.save_settings();
        }

        #[cfg(feature = "gles2")]
        let gles2_rendered = if self.gles2_effect.is_some() && self.gles2_state.is_some() {
            let gl = self.gles2_state.as_ref().unwrap().gl.clone();
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let keep_running = match self
                .gles2_effect
                .as_mut()
                .unwrap()
                .render_frame(&gl, &state)
            {
                Ok(v) => v,
                Err(e) => {
                    log::error!("GLES 2.0 render error: {e}");
                    true
                }
            };
            drop(state);
            if keep_running {
                if let Err(e) = self.gles2_state.as_mut().unwrap().present() {
                    log::error!("GLES 2.0 present error: {e}");
                }
            } else {
                event_loop.exit();
            }
            true
        } else {
            false
        };
        #[cfg(not(feature = "gles2"))]
        let gles2_rendered = false;

        // Throttle the control-window UI (and preview texture updates) to ~30 Hz.
        let ui_due =
            self.ui_needs_redraw || now.duration_since(self.last_ui_render) >= UI_RENDER_INTERVAL;

        let pre_render = std::time::Instant::now();
        if !gles2_rendered {
            if let Some(ref mut engine) = self.output_engine {
                engine.render(self.output_occluded, &mut self.app_state);
                if ui_due {
                    self.update_preview_textures();
                }
            }
        }

        // Write CPU update time into performance metrics.
        {
            let cpu_update_ms = pre_render.duration_since(frame_start).as_secs_f32() * 1000.0;
            if let Ok(state) = self.shared_state.lock() {
                if let Ok(mut perf) = state.performance.lock() {
                    perf.cpu_update_ms = cpu_update_ms;
                }
            }
        }

        #[cfg(feature = "projection")]
        if let (Some(sub), Some(device), Some(queue), Some(engine)) = (
            self.projection_subsystem.as_ref(),
            self.wgpu_device.as_deref(),
            self.wgpu_queue.as_deref(),
            self.output_engine.as_ref(),
        ) {
            let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
            sub.render(
                device,
                queue,
                &engine.render_target.view,
                Some(&engine.render_target.texture),
                [engine.render_target.width, engine.render_target.height],
            );
        }

        if self.control_visible && ui_due {
            self.last_ui_render = now;
            self.ui_needs_redraw = false;
            if self.use_egui {
                #[cfg(feature = "egui")]
                if let (Some(window), Some(ref mut renderer), Some(ref mut gui)) = (
                    self.control_window.as_ref(),
                    self.egui_renderer.as_mut(),
                    self.egui_control_gui.as_mut(),
                ) {
                    let scale_factor = window.scale_factor();
                    let window_size = window.inner_size();
                    let logical_width = window_size.width as f32 / scale_factor as f32;
                    let logical_height = window_size.height as f32 / scale_factor as f32;
                    renderer.set_display_size(logical_width, logical_height);

                    let app_state = &mut self.app_state as &mut dyn std::any::Any;
                    if let Err(err) = renderer.render_frame(|ctx| gui.build_ui(ctx, app_state)) {
                        log::error!("egui render error: {}", err);
                    }
                }
            } else if let (Some(window), Some(ref mut renderer), Some(ref mut gui)) = (
                self.control_window.as_ref(),
                self.imgui_renderer.as_mut(),
                self.control_gui.as_mut(),
            ) {
                let scale_factor = window.scale_factor();
                let window_size = window.inner_size();
                let logical_width = window_size.width as f32 / scale_factor as f32;
                let logical_height = window_size.height as f32 / scale_factor as f32;
                renderer.set_display_size(logical_width, logical_height);

                let app_state = &mut self.app_state as &mut dyn std::any::Any;
                if let Err(err) = renderer.render_frame(|ui| gui.build_ui(ui, app_state)) {
                    log::error!("ImGui render error: {}", err);
                }
            }
        }

        // Use Poll so the event loop wakes every frame; actual pacing is handled
        // by the renderer's next_render_time software cap and/or the surface
        // present mode (AutoVsync/Fifo). The old WaitUntil throttle fought against
        // hardware vsync and caused beat-frequency jitter on high-refresh displays.
        event_loop.set_control_flow(ControlFlow::Poll);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        log::info!("Event loop exiting — shutting down");
        self.save_settings();

        if let Some(ref mut analyzer) = self.audio_analyzer {
            analyzer.stop();
        }
        #[cfg(feature = "link")]
        if let Some(ref mut manager) = self.link_manager {
            manager.disable();
        }
        // ProDJ Link requires no explicit cleanup — sockets close on process exit.
        if let Some(ref mut manager) = self.midi_manager {
            manager.disconnect();
        }
        if let Some(ref mut server) = self.osc_server {
            server.stop();
        }
        if let Some(ref mut server) = self.web_server {
            server.stop();
        }
        if let Some(ref mut manager) = self.input_manager {
            manager.stop();
        }
        if let Some(ref mut manager) = self.second_input_manager {
            manager.stop();
        }
        if let Some(ref mut engine) = self.output_engine {
            engine.drain_readback();
        }

        log::info!("Shutdown complete");
    }
}
