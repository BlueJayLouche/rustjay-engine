//! Simple Syphon Test - Renders a colored texture and publishes to Syphon

use std::time::Instant;

fn main() {
    env_logger::init();
    
    println!("=== Syphon wgpu Test ===\n");
    
    // Check if Syphon is available
    if !syphon_wgpu::is_available() {
        eprintln!("Syphon not available on this system");
        return;
    }
    println!("✓ Syphon available");
    
    // Set up wgpu
    let (device, queue) = match setup_wgpu() {
        Ok((d, q)) => {
            println!("✓ wgpu initialized");
            (d, q)
        }
        Err(e) => {
            eprintln!("✗ Failed to initialize wgpu: {}", e);
            return;
        }
    };
    
    let width = 640u32;
    let height = 480u32;
    
    // Create Syphon output
    println!("\nCreating Syphon output ({}x{})...", width, height);
    let mut syphon = syphon_wgpu::SyphonWgpuOutput::new("Syphon Test", &device, &queue, width, height)
        .expect("Failed to create Syphon output");
    println!("✓ Syphon output created");
    
    // List other servers
    println!("\nOther Syphon servers:");
    let servers = syphon_wgpu::list_servers();
    if servers.is_empty() {
        println!("  (none found)");
    } else {
        for name in servers.iter().filter(|n| n != &"Syphon Test") {
            println!("  - {}", name);
        }
    }
    
    // Create a simple colored texture
    println!("\nCreating test texture...");
    let texture = create_test_texture(&device, width, height);
    println!("✓ Test texture created");
    
    // Main loop
    println!("\n🎬 Broadcasting to Syphon...");
    println!("Open 'Syphon Simple Client' to view.");
    println!("Press Ctrl+C to exit.\n");
    
    let start = Instant::now();
    let mut frame = 0u64;
    
    loop {
        let time = start.elapsed().as_secs_f32();
        
        // Update texture with animated color
        update_texture(&device, &queue, &texture, width, height, time);
        
        // Publish to Syphon
        syphon.publish(&texture, &device, &queue);
        
        frame += 1;
        if frame % 60 == 0 {
            let fps = frame as f64 / start.elapsed().as_secs_f64();
            println!("📡 {} clients | {} frames | {:.1} FPS", 
                syphon.client_count(), frame, fps);
        }
        
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn setup_wgpu() -> Result<(wgpu::Device, wgpu::Queue), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    
    // Try high performance first (discrete GPU), then fall back
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    })).map_err(|e| format!("Failed to find adapter: {:?}", e))?;
    
    println!("  Adapter: {:?}", adapter.get_info().name);
    
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("Syphon Test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::default(),
        },
    ))?;
    
    Ok((device, queue))
}

fn create_test_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn update_texture(
    _device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    time: f32,
) {
    // Create animated color data
    let mut data = vec![0u8; (width * height * 4) as usize];
    
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            
            // Animated gradient
            let r = ((x as f32 / width as f32 + time.sin()) * 127.5 + 127.5) as u8;
            let g = ((y as f32 / height as f32 + (time + 2.0).sin()) * 127.5 + 127.5) as u8;
            let b = (((x + y) as f32 / (width + height) as f32 + (time + 4.0).sin()) * 127.5 + 127.5) as u8;
            
            data[idx] = b;     // B
            data[idx + 1] = g; // G
            data[idx + 2] = r; // R
            data[idx + 3] = 255; // A
        }
    }
    
    // Write to texture
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
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
