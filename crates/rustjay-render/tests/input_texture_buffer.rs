//! `InputTexture::update_from_buffer` with a padded row stride.
//!
//! The NDI receive thread writes frames into mapped buffers at wgpu's copy
//! alignment, so rows sit further apart than `width * 4` whenever the frame's
//! own row length isn't a multiple of 256. Getting that stride wrong skews the
//! image by a few pixels a row rather than failing outright, which is the kind
//! of thing that survives a glance at the output.
//!
//! GPU test: opt in with `RUSTJAY_GPU_TESTS=1`, like the ISF pixel tests.

use std::sync::Arc;

use rustjay_render::InputTexture;

// 70 * 4 = 280 bytes of pixels a row, padded to 512. The padding is not a whole
// number of pixels, so a stride bug cannot accidentally line up.
const WIDTH: u32 = 70;
const HEIGHT: u32 = 4;
const BYTES_PER_ROW: u32 = 512;

fn init_gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    if std::env::var("RUSTJAY_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("RUSTJAY_GPU_TESTS != 1 — skipping input texture GPU test");
        return None;
    }
    Some(pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .expect("no wgpu adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                label: Some("InputTexture Buffer Test Device"),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .expect("no wgpu device")
    }))
}

/// BGRA for pixel (x, y): blue carries x, green carries y, so a row or column
/// slip shows up as a specific wrong number rather than "looks odd".
fn expected(x: u32, y: u32) -> [u8; 4] {
    [x as u8, y as u8, 0x80, 0xff]
}

#[test]
fn update_from_buffer_honours_the_row_stride() {
    let Some((device, queue)) = init_gpu() else {
        return;
    };

    // Fill a mapped buffer the way the NDI receive thread does: pixels packed to
    // width, rows BYTES_PER_ROW apart, the padding left as a value that must not
    // reach the texture.
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staged frame"),
        size: u64::from(BYTES_PER_ROW) * u64::from(HEIGHT),
        usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    {
        let total = (BYTES_PER_ROW * HEIGHT) as usize;
        let mut bytes = vec![0x5au8; total];
        for y in 0..HEIGHT {
            let row = (y * BYTES_PER_ROW) as usize;
            for x in 0..WIDTH {
                let at = row + (x * 4) as usize;
                bytes[at..at + 4].copy_from_slice(&expected(x, y));
            }
        }
        let mut view = staging
            .get_mapped_range_mut(..)
            .expect("mapped at creation");
        view.slice(..total).copy_from_slice(&bytes);
    }
    staging.unmap();

    let mut input = InputTexture::new(Arc::new(device.clone()), Arc::new(queue.clone()));
    input.update_from_buffer(&staging, BYTES_PER_ROW, WIDTH, HEIGHT);

    // Read the texture back. copy_texture_to_buffer wants its own 256-aligned
    // row, which is the same 512 here.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(BYTES_PER_ROW) * u64::from(HEIGHT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let texture = &input.texture.as_ref().expect("texture created").texture;
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(BYTES_PER_ROW),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let mapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = mapped.clone();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |_| {
        flag.store(true, std::sync::atomic::Ordering::Release);
    });
    while !mapped.load(std::sync::atomic::Ordering::Acquire) {
        device.poll(wgpu::PollType::Poll).ok();
        std::thread::yield_now();
    }
    let data = readback
        .slice(..)
        .get_mapped_range()
        .expect("mapped by map_async")
        .to_vec();

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let at = (y * BYTES_PER_ROW + x * 4) as usize;
            assert_eq!(
                &data[at..at + 4],
                &expected(x, y),
                "pixel ({x}, {y}) — a row stride bug shifts pixels within the row"
            );
        }
    }
}
