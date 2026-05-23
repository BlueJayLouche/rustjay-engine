use super::{App, WindowAction};
use rustjay_gui::{ControlGui, ImGuiRenderer};
use rustjay_render::WgpuEngine;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowAttributes;
use rustjay_core::EffectPlugin;

impl<P: EffectPlugin> ApplicationHandler<WindowAction> for App<P> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.wgpu_instance.is_none() {
            let backends = if cfg!(target_os = "macos") {
                wgpu::Backends::METAL
            } else {
                wgpu::Backends::all()
            };
            self.wgpu_instance = Some(wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            }));
        }
        let Some(instance) = self.wgpu_instance.as_ref() else { return };

        if self.output_window.is_none() {
            let (output_width, output_height, fullscreen) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.output_width, state.output_height, state.output_fullscreen)
            };

            let window_attrs = WindowAttributes::default()
                .with_title("RustJay Output")
                .with_inner_size(winit::dpi::LogicalSize::new(output_width, output_height))
                .with_resizable(true)
                .with_decorations(true);

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
            window.set_cursor_visible(false);
            self.output_window = Some(Arc::clone(&window));

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

                    if let (Some(ref mut manager), Some(ref device), Some(ref queue)) =
                        (self.input_manager.as_mut(), self.wgpu_device.as_ref(), self.wgpu_queue.as_ref())
                    {
                        manager.initialize(device, queue);
                        log::info!("InputManager initialized with GPU resources");
                    }
                    if let (Some(ref mut manager), Some(ref device), Some(ref queue)) =
                        (self.second_input_manager.as_mut(), self.wgpu_device.as_ref(), self.wgpu_queue.as_ref())
                    {
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
        }

        if self.control_window.is_none() {
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
                match pollster::block_on(ImGuiRenderer::new(
                    instance, adapter, device, queue, window, scale_factor,
                )) {
                    Ok(mut renderer) => {
                        match ControlGui::new(Arc::clone(&self.shared_state)) {
                            Ok(mut gui) => {
                                let input_preview_id = renderer.create_preview_texture(1920, 1080);
                                let output_preview_id = renderer.create_preview_texture(1920, 1080);
                                gui.set_input_preview_texture(input_preview_id);
                                gui.set_output_preview_texture(output_preview_id);
                                log::info!("Created preview textures");

                                // Move custom tabs into the GUI
                                gui.custom_tabs = std::mem::take(&mut self.custom_tabs);

                                self.control_gui = Some(gui);
                                {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.input_command = rustjay_core::InputCommand::RefreshDevices;
                                }
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
                    WindowEvent::KeyboardInput { event, .. } => {
                        if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Shift) = &event.logical_key {
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
                                winit::keyboard::Key::Named(named) => {
                                    if self.shift_pressed {
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
                                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                                let _ = bank.apply_slot(s, &mut state);
                                            }
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
                    WindowEvent::Resized(size) => {
                        if !self.output_occluded {
                            if let Some(ref mut engine) = self.output_engine {
                                engine.resize(size.width, size.height);
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
        }

        if let Some(control_window) = self.control_window.as_ref() {
            if window_id == control_window.id() {
                if let Some(ref mut renderer) = self.imgui_renderer {
                    let winit_event = winit::event::Event::WindowEvent { window_id, event: event.clone() };
                    renderer.handle_event(&winit_event);
                }

                match event {
                    WindowEvent::CloseRequested => {
                        let window = Arc::clone(control_window);
                        self.save_settings();
                        window.set_visible(false);
                        self.control_visible = false;
                        log::info!("Control window hidden");
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Shift) = &event.logical_key {
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
                    WindowEvent::Resized(size) => {
                        if self.control_visible {
                            if let Some(ref mut renderer) = self.imgui_renderer {
                                renderer.resize(size.width, size.height);
                            }
                        }
                    }
                    WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                        if self.control_visible {
                            if let Some(ref mut renderer) = self.imgui_renderer {
                                renderer.set_scale_factor(scale_factor);
                                let window_size = control_window.inner_size();
                                let logical_width = window_size.width as f32 / scale_factor as f32;
                                let logical_height = window_size.height as f32 / scale_factor as f32;
                                renderer.set_display_size(logical_width, logical_height);
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = std::time::Instant::now();
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

        if let Some(ref mut engine) = self.output_engine {
            engine.render(self.output_occluded, &mut self.app_state);
            self.update_preview_textures();
        }
        if self.control_visible {
            if let (Some(ref window), Some(ref mut renderer), Some(ref mut gui)) =
                (self.control_window.as_ref(), self.imgui_renderer.as_mut(), self.control_gui.as_mut())
            {
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

        let target_fps = self.shared_state.lock().unwrap_or_else(|e| e.into_inner()).target_fps;
        let target_frame_dur = std::time::Duration::from_micros(1_000_000 / target_fps as u64);
        let elapsed = now.elapsed();
        if elapsed < target_frame_dur {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + (target_frame_dur - elapsed),
            ));
        }
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
