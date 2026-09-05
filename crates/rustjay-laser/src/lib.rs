//! Render MadMapper laser materials to 2D paths.
//!
//! A laser material is not a pixel shader: `rustjay-isf` compiles it into a
//! fragment pass over a `POINT_COUNT`-wide, 3-row target where each column is
//! one sample of a path — see [`rustjay_isf::compile::LASER_ROWS`]. This crate
//! owns that target, drives the shader through the ordinary [`IsfEffect`]
//! runtime, reads the result back, and hands out a [`LaserFrame`].
//!
//! What it deliberately does not do is decide *when* to render. A laser deck
//! runs on the scanner's clock rather than the engine's, so the host asks
//! [`LaserDeck::due`] once per engine frame and renders only when it is.

/// Streaming to real hardware. Behind the `dac` feature: it pulls in libusb
/// and CMake, and everything up to the point list works without it.
#[cfg(feature = "dac")]
pub mod dac;
pub mod calibrate;
pub mod frame;
pub mod geometry;
pub mod optimise;
pub mod safety;

pub use frame::{LaserFrame, LaserPoint};
pub use geometry::Geometry;
pub use optimise::Optimiser;
pub use safety::{Blocked, Safety};

use std::path::Path;
use std::time::{Duration, Instant};

use rustjay_core::{EffectPlugin, EngineState};
use rustjay_isf::{IsfEffect, IsfState, compile::LASER_ROWS};

/// The target's format. Positions are -1..1 and user data is arbitrary, so an
/// 8-bit unorm target would clamp both and quantise the beam to 256 steps —
/// against a DAC that resolves 65,536.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// wgpu requires a buffer copy's row stride to be a multiple of this.
const COPY_ALIGN: usize = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;

/// Fewest points worth asking a material for — two make a line.
const MIN_POINTS: u32 = 2;

/// Most points to ask for regardless of arithmetic, so a mistyped refresh
/// cannot allocate something absurd. 16k is the largest `POINT_COUNT` any
/// shader in MadMapper's corpus declares.
const MAX_POINTS: u32 = 16_384;

/// Share of the budget held back for the points the optimiser inserts —
/// blanking transits between strokes and dwell at corners.
///
/// ponytail: a flat fraction, not a measurement. It is only wrong in one
/// direction — too little headroom and the real refresh dips below the
/// requested one. Upgrade path is to measure the optimiser's actual expansion
/// over a frame and feed it back.
const OPTIMISER_HEADROOM: f32 = 0.25;

/// What the scanner can draw, and how often.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Sample rate the projector's galvos can track. 30k is the rate the ILDA
    /// test pattern is specified at; cheap scanners manage 20k.
    pub points_per_second: u32,
    /// How often the whole path should be redrawn. Below about 20 it flickers.
    pub refresh_hz: f32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { points_per_second: 30_000, refresh_hz: 30.0 }
    }
}

impl Budget {
    /// How many samples to ask a material for.
    ///
    /// The scanner can only draw `points_per_second / refresh_hz` of them per
    /// pass, so that — less headroom for the optimiser — is the ceiling. A
    /// material that asked for fewer keeps its own number: a straight line
    /// declaring two points gains nothing from a thousand.
    ///
    /// Every material in MadMapper's corpus divides by the count it is handed,
    /// so a smaller budget draws the same shape at a lower density rather than
    /// a different shape.
    pub fn points(&self, declared: Option<u32>) -> u32 {
        let affordable = if self.refresh_hz > 0.0 {
            (self.points_per_second as f32 / self.refresh_hz) * (1.0 - OPTIMISER_HEADROOM)
        } else {
            MAX_POINTS as f32
        };
        let affordable = (affordable as u32).clamp(MIN_POINTS, MAX_POINTS);
        declared.map_or(affordable, |d| d.clamp(MIN_POINTS, affordable))
    }

    /// How long one pass over the path takes.
    pub fn period(&self) -> Duration {
        Duration::from_secs_f32(1.0 / self.refresh_hz.max(1.0))
    }
}

/// A laser material, rendered and read back.
///
/// The two targets ping-pong: the shader draws into one while reading the
/// other as `mm_LastFrameData`, which is how the 45% of materials that damp or
/// trail get their history.
pub struct LaserDeck {
    /// Shown in the UI; the shader's file stem.
    pub name: String,
    /// Set when the shader would not compile, for the host to display.
    pub error: Option<String>,
    effect: IsfEffect,
    state: IsfState,
    budget: Budget,
    /// Fixed when the deck loads: it is also the targets' width, and every
    /// feedback material indexes `mm_LastFrameData` by it. Changing it means
    /// reallocating and starting the history over — see [`LaserDeck::retune`].
    point_count: u32,
    targets: Vec<Target>,
    /// Which target the last frame was drawn into.
    front: usize,
    readback: Readback,
    /// As the shader produced it.
    latest: LaserFrame,
    /// The same path with the settling points a scanner needs.
    optimised: LaserFrame,
    next_due: Option<Instant>,
    /// Insertions made on the way out. Seeded from the material's
    /// `RENDER_SETTINGS`, then the operator's to adjust.
    pub optimiser: Optimiser,
    /// Where the scan field lands in the room. Applied after the optimiser and
    /// before the gate, so what [`Safety`] judges is what the galvos receive —
    /// a correction that shrank the picture would otherwise pass a scan-fail
    /// check it should have failed.
    pub geometry: Geometry,
    /// The arm gate and scan-fail guard. Disarmed until someone says otherwise.
    pub safety: Safety,
}

/// One side of the ping-pong.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl LaserDeck {
    /// Load a laser material.
    ///
    /// Fails if the shader is not one — a video material compiles fine but
    /// would write colours into a buffer read as coordinates.
    pub fn from_path(path: &Path, budget: Budget) -> anyhow::Result<Self> {
        let src = std::fs::read_to_string(path)?;
        let declared = rustjay_isf::header::render_settings(&src).point_count;
        let point_count = budget.points(declared);

        let mut effect = IsfEffect::from_path(path)?;
        effect.offscreen_format = Some(TARGET_FORMAT);
        effect.offscreen_size = Some([point_count, LASER_ROWS]);
        let state = effect.default_state();

        Ok(Self {
            name: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            error: None,
            effect,
            state,
            budget,
            point_count,
            targets: Vec::new(),
            front: 0,
            readback: Readback::default(),
            latest: LaserFrame::default(),
            optimised: LaserFrame::default(),
            next_due: None,
            optimiser: Optimiser::default(),
            geometry: Geometry::identity(),
            safety: Safety::default(),
        })
    }

    /// Build the shader pipeline and the targets.
    pub fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        EffectPlugin::init(&mut self.effect, device, queue);
        self.error = self.effect.transpile_error.clone();
        if self.error.is_none()
            && self.effect.manifest().is_some_and(|m| m.laser.is_none())
        {
            self.error = Some("not a laser material: it has no laserMaterialFunc".into());
        }
        if self.error.is_some() {
            return;
        }
        if let Some(settings) = self.effect.manifest().and_then(|m| m.laser.clone()) {
            self.optimiser = Optimiser::from_render_settings(&settings);
        }
        self.targets = (0..2).map(|i| Target::new(device, self.point_count, i)).collect();
    }

    /// Points the material asks for each pass.
    pub fn point_count(&self) -> u32 {
        self.point_count
    }

    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// Change the scanner settings, which resizes the targets and starts the
    /// feedback history over. Deliberately not something to do per frame.
    pub fn retune(&mut self, budget: Budget, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.budget = budget;
        let declared = self.effect.manifest().and_then(|m| {
            m.laser.as_ref().and_then(|s| s.point_count)
        });
        self.point_count = budget.points(declared);
        self.effect.offscreen_size = Some([self.point_count, LASER_ROWS]);
        self.readback = Readback::default();
        self.latest = LaserFrame::default();
        self.next_due = None;
        self.init(device, queue);
    }

    /// Whether the next pass is due, on the scanner's clock rather than the
    /// engine's. Rendering faster than this would advance the material's time
    /// and feedback history past what the beam actually draws.
    pub fn due(&self, now: Instant) -> bool {
        self.next_due.is_none_or(|due| now >= due)
    }

    /// Draw one pass and start reading it back.
    ///
    /// Nothing here blocks: the result of *this* call arrives from a later
    /// [`LaserDeck::poll`], so the newest frame is always a pass or two old.
    /// At 30 Hz that is tens of milliseconds, which no galvo can tell from
    /// live, and it keeps the render thread off the GPU's schedule.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        engine: &EngineState,
        quad: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
    ) {
        if self.error.is_some() || self.targets.len() < 2 {
            return;
        }
        let now = Instant::now();
        // Re-anchor rather than accumulate when we have fallen behind, so a
        // stalled engine does not then render a burst of catch-up passes.
        self.next_due = Some(match self.next_due {
            Some(due) if now < due + self.budget.period() => due + self.budget.period(),
            _ => now + self.budget.period(),
        });

        let back = 1 - self.front;
        let (targets_back, targets_front) = (&self.targets[back], &self.targets[self.front]);
        {
            let mut ctx = rustjay_core::RenderHookCtx {
                encoder,
                device,
                queue,
                // The material's "input" is its own previous pass, which is how
                // `mm_LastFrameData` reaches it — see `IsfEffect::init`.
                input: Some(rustjay_core::EffectInput {
                    view: &targets_front.view,
                    sampler,
                    generation: 0,
                    texture: Some(&targets_front.texture),
                }),
                target_view: &targets_back.view,
                engine_state: engine,
                vertex_buffer: quad,
            };
            EffectPlugin::render(&mut self.effect, &mut ctx, &mut self.state);
        }
        self.readback.copy_from(device, encoder, &self.targets[back], self.point_count);
        self.front = back;
    }

    /// Collect any completed readback. Call once per engine frame.
    pub fn poll(&mut self, device: &wgpu::Device) {
        if let Some(frame) = self.readback.take(device, self.point_count) {
            let mut optimised = self.optimiser.run(&frame);
            self.geometry.apply(&mut optimised);
            self.optimised = optimised;
            self.latest = frame;
        }
    }

    /// The most recently completed pass, as the shader drew it. This is what a
    /// preview should show — the geometry the material describes, without the
    /// settling points that exist only for the mirrors.
    pub fn frame(&self) -> &LaserFrame {
        &self.latest
    }

    /// The frame to send to a DAC, or `None` when the gate is holding output
    /// dark — [`Safety::blocked`] says why.
    ///
    /// Everything upstream runs whether or not this returns anything, so the
    /// preview keeps working while the output is disarmed.
    pub fn output(&mut self) -> Option<&LaserFrame> {
        let ok = self.error.is_none();
        self.safety.gate(&self.optimised, ok)
    }

    /// The shader's own parameters, for the host to register and map.
    pub fn parameters(&self) -> Vec<rustjay_core::ParameterDescriptor> {
        self.effect.parameters()
    }

    pub fn state_mut(&mut self) -> &mut IsfState {
        &mut self.state
    }
}

impl Target {
    fn new(device: &wgpu::Device, point_count: u32, index: usize) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if index == 0 { "Laser Target A" } else { "Laser Target B" }),
            size: wgpu::Extent3d {
                width: point_count,
                height: LASER_ROWS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

/// Row stride of the readback buffer: the texture's width in bytes, padded up
/// to what wgpu will copy.
fn row_pitch(point_count: u32) -> usize {
    (point_count as usize * frame::TEXEL_BYTES).next_multiple_of(COPY_ALIGN)
}

/// A two-slot readback, so the GPU fills one while the CPU reads the other.
///
/// Deliberately its own rather than the pool in `rustjay-io`: that one is
/// shaped around video frames — BGRA, output resolution, feeding NDI and the
/// recorder — and a laser pass is 2 rows of a few hundred texels. Widening it
/// would mean editing the video output's hot path for this.
#[derive(Default)]
struct Readback {
    slots: Vec<Slot>,
    next: usize,
}

struct Slot {
    buffer: wgpu::Buffer,
    /// Set while a `map_async` is outstanding.
    pending: Option<std::sync::mpsc::Receiver<bool>>,
}

impl Readback {
    fn copy_from(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &Target,
        point_count: u32,
    ) {
        let pitch = row_pitch(point_count);
        let size = (pitch * frame::READBACK_ROWS as usize) as u64;
        if self.slots.len() < 2 || self.slots[0].buffer.size() != size {
            self.slots = (0..2)
                .map(|_| Slot {
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Laser Readback"),
                        size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    pending: None,
                })
                .collect();
            self.next = 0;
        }
        // Skip rather than queue behind an outstanding map: dropping a pass is
        // better than growing latency without bound.
        if self.slots[self.next].pending.is_some() {
            return;
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.slots[self.next].buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch as u32),
                    rows_per_image: Some(frame::READBACK_ROWS),
                },
            },
            wgpu::Extent3d {
                width: point_count,
                // Only position and colour come back; the user-data row stays
                // on the GPU as the next pass's history.
                height: frame::READBACK_ROWS,
                depth_or_array_layers: 1,
            },
        );
        let (tx, rx) = std::sync::mpsc::channel();
        self.slots[self.next]
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res.is_ok());
            });
        self.slots[self.next].pending = Some(rx);
        self.next = (self.next + 1) % 2;
    }

    /// The oldest completed readback, or `None` while the GPU is still working.
    fn take(&mut self, device: &wgpu::Device, point_count: u32) -> Option<LaserFrame> {
        device.poll(wgpu::PollType::Poll).ok();
        let pitch = row_pitch(point_count);
        for slot in &mut self.slots {
            let Some(rx) = &slot.pending else { continue };
            match rx.try_recv() {
                Ok(true) => {
                    let frame = {
                        let view = slot.buffer.slice(..).get_mapped_range().ok()?;
                        LaserFrame::decode(&view, point_count as usize, pitch)
                    };
                    slot.buffer.unmap();
                    slot.pending = None;
                    return Some(frame);
                }
                // Mapping failed — free the slot and try again next pass.
                Ok(false) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    slot.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_what_the_scanner_can_draw() {
        let budget = Budget { points_per_second: 30_000, refresh_hz: 30.0 };

        // 1000 a pass, less a quarter held back for the optimiser.
        assert_eq!(budget.points(None), 750);
    }

    #[test]
    fn a_material_asking_for_less_keeps_its_own_count() {
        let budget = Budget { points_per_second: 30_000, refresh_hz: 30.0 };

        assert_eq!(budget.points(Some(500)), 500);
        assert_eq!(budget.points(Some(2)), 2);
    }

    // 8192 at 30k would redraw 3.7 times a second.
    #[test]
    fn a_material_asking_for_more_than_the_scanner_can_draw_is_capped() {
        let budget = Budget { points_per_second: 30_000, refresh_hz: 30.0 };

        assert_eq!(budget.points(Some(8192)), 750);
    }

    #[test]
    fn a_faster_scanner_affords_more_points_at_the_same_refresh() {
        let slow = Budget { points_per_second: 20_000, refresh_hz: 30.0 };
        let fast = Budget { points_per_second: 40_000, refresh_hz: 30.0 };

        assert!(fast.points(None) > slow.points(None));
    }

    #[test]
    fn a_nonsense_refresh_cannot_allocate_something_absurd() {
        let stopped = Budget { points_per_second: 30_000, refresh_hz: 0.0 };
        let frantic = Budget { points_per_second: 30_000, refresh_hz: 100_000.0 };

        assert_eq!(stopped.points(None), MAX_POINTS);
        assert_eq!(frantic.points(None), MIN_POINTS);
    }

    #[test]
    fn the_readback_stride_is_something_wgpu_will_copy() {
        for points in [2, 500, 750, 1000, 16_384] {
            assert_eq!(row_pitch(points) % COPY_ALIGN, 0, "{points}");
            assert!(row_pitch(points) >= points as usize * frame::TEXEL_BYTES);
        }
    }
}
