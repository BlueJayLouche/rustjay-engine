//! Video output thread — runs a dedicated winit event loop + wgpu renderer.
//!
//! Communicates with the main thread via `EventLoopProxy<VideoCommand>`.
//! A/V sync uses the provided audio clock closure as the master.

use crate::{OutputWindow, Renderer, Texture, VideoFrame, VideoSource};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

/// Commands sent from the main thread to the video output thread.
#[derive(Debug, Clone)]
pub enum VideoCommand {
    /// Open a video file and start playback.
    Open(String),
    /// Stop playback and clear the frame.
    Stop,
}

/// Handle used by the main thread to control video playback.
pub struct VideoOutputHandle {
    proxy: winit::event_loop::EventLoopProxy<VideoCommand>,
}

impl VideoOutputHandle {
    /// Send a command to the video thread.
    pub fn send(&self, cmd: VideoCommand) {
        if let Err(e) = self.proxy.send_event(cmd) {
            log::error!("Video event loop closed: {e:?}");
        }
    }

    /// Open a video file.
    pub fn open(&self, path: &str) {
        self.send(VideoCommand::Open(path.to_string()));
    }

    /// Stop playback.
    pub fn stop(&self) {
        self.send(VideoCommand::Stop);
    }
}

/// Function that returns the current audio master clock time.
pub type ClockFn = Arc<dyn Fn() -> Duration + Send + Sync>;

/// Spawn the video output thread. Returns a handle for sending commands.
pub fn spawn_video_output(clock: ClockFn) -> anyhow::Result<VideoOutputHandle> {
    let event_loop = EventLoop::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    std::thread::Builder::new()
        .name("video-output".into())
        .spawn(move || {
            let mut app = VideoApp::new(clock);
            event_loop.set_control_flow(ControlFlow::Poll);
            if let Err(e) = event_loop.run_app(&mut app) {
                log::error!("Video event loop error: {e}");
            }
        })?;

    Ok(VideoOutputHandle { proxy })
}

struct VideoApp {
    clock: ClockFn,
    window: Option<OutputWindow>,
    renderer: Option<Renderer>,
    texture: Option<Texture>,
    video_source: Option<VideoSource>,
    current_frame: Option<VideoFrame>,
    pending_frame: Option<VideoFrame>,
    /// Audio clock value at the moment playback started.
    start_clock: Option<Duration>,
    /// Wall-clock instant captured at start (for frame pacing).
    start_instant: Option<Instant>,
}

impl VideoApp {
    fn new(clock: ClockFn) -> Self {
        Self {
            clock,
            window: None,
            renderer: None,
            texture: None,
            video_source: None,
            current_frame: None,
            pending_frame: None,
            start_clock: None,
            start_instant: None,
        }
    }

    /// Ensure the output window (and dependent wgpu objects) exist.
    fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match OutputWindow::new(event_loop) {
            Ok(mut win) => {
                let texture = Texture::new(&win.device, 1920, 1080);
                let renderer = Renderer::new(&win.device, texture.bind_group_layout());
                win.window.request_redraw();
                self.window = Some(win);
                self.texture = Some(texture);
                self.renderer = Some(renderer);
            }
            Err(e) => {
                log::error!("Failed to create video output window: {e}");
            }
        }
    }

    /// Open a video file and prepare for playback.
    fn open(&mut self, path: &str, event_loop: &ActiveEventLoop) {
        self.ensure_window(event_loop);

        match VideoSource::open(path, 1920, 1080) {
            Ok(mut source) => {
                // Pre-decode first frame
                self.pending_frame = source.read_frame();
                self.video_source = Some(source);
                self.start_clock = Some((self.clock)());
                self.start_instant = Some(Instant::now());
                log::info!("Video playback started: {}", path);
            }
            Err(e) => {
                log::error!("Failed to open video {}: {e}", path);
                self.video_source = None;
            }
        }
    }

    /// Stop playback.
    fn stop(&mut self) {
        self.video_source = None;
        self.current_frame = None;
        self.pending_frame = None;
        self.start_clock = None;
        self.start_instant = None;
        log::info!("Video playback stopped");
    }

    /// Decode the next frame into `pending_frame` if empty.
    fn decode_next(&mut self) {
        if self.pending_frame.is_some() {
            return;
        }
        if let Some(source) = self.video_source.as_mut() {
            self.pending_frame = source.read_frame();
            if self.pending_frame.is_none() {
                // EOF — optionally loop or stop. For now, stop.
                log::info!("Video reached EOF");
            }
        }
    }

    /// Check A/V sync and promote pending → current when due.
    fn update_sync(&mut self) {
        let Some(start) = self.start_clock else { return };
        let audio_now = (self.clock)().saturating_sub(start);

        while let Some(ref pending) = self.pending_frame {
            // Present frame when its PTS is within 5 ms of (or behind) the audio clock.
            const SYNC_THRESHOLD: f64 = 0.005;
            if pending.pts as f64 <= audio_now.as_secs_f64() + SYNC_THRESHOLD {
                self.current_frame = self.pending_frame.take();
                if let Some(win) = self.window.as_ref() {
                    win.window.request_redraw();
                }
            } else {
                break;
            }
        }
    }

    /// Upload the current frame and render it.
    fn render(&mut self) {
        let Some(win) = self.window.as_ref() else { return };
        let Some(renderer) = self.renderer.as_ref() else { return };
        let Some(texture) = self.texture.as_mut() else { return };

        // Upload new frame if available
        if let Some(frame) = self.current_frame.as_ref() {
            texture.upload(&win.queue, frame);
        }

        let output = match win.surface.get_current_texture() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Surface acquire failed: {e}");
                return;
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = win.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("video-encoder"),
        });

        renderer.render(&mut encoder, &view, texture.current_bind_group());
        win.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}

impl ApplicationHandler<VideoCommand> for VideoApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // Window is created lazily on first Open command.
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: VideoCommand) {
        match event {
            VideoCommand::Open(path) => self.open(&path, event_loop),
            VideoCommand::Stop => self.stop(),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.window = None;
                self.renderer = None;
                self.texture = None;
            }
            WindowEvent::Resized(size) => {
                if let Some(win) = self.window.as_mut() {
                    win.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                self.render();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.decode_next();
        self.update_sync();
    }
}
