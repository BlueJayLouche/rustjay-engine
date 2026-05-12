//! Syphon Input Test - Tests the full input pipeline
//!
//! This example mimics how rustjay_waaaves uses Syphon input:
//! - Background thread for frame polling
//! - Channel for frame delivery
//! - BGRA to RGBA conversion
//!
//! Usage:
//!   cargo run --example input_test -- "Server Name"

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    env_logger::init();
    
    println!("=== Syphon Input Pipeline Test ===\n");
    
    if !syphon_core::is_available() {
        eprintln!("Error: Syphon framework is not available.");
        std::process::exit(1);
    }
    
    let server_name = match std::env::args().nth(1) {
        Some(name) => name,
        None => {
            println!("Discovering servers...");
            let servers = syphon_core::SyphonServerDirectory::servers();
            if servers.is_empty() {
                eprintln!("No servers found.");
                std::process::exit(1);
            }
            println!("Found: {}", servers[0].display_name());
            println!("Usage: cargo run --example input_test -- \"Server Name\"");
            servers[0].name.clone()
        }
    };
    
    println!("Connecting to: '{}'\n", server_name);
    
    // Create channel for frame delivery (like rustjay_waaaves does)
    let (tx, rx) = crossbeam::channel::bounded(5);
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);
    
    // Spawn background thread (like rustjay_waaaves does)
    let server_name_clone = server_name.clone();
    let handle = thread::spawn(move || {
        receive_thread(server_name_clone, tx, running_clone);
    });
    
    // Main thread: consume frames
    println!("Receiving frames for 10 seconds...\n");
    let start = Instant::now();
    let mut received = 0;
    
    while start.elapsed() < Duration::from_secs(10) {
        match rx.try_recv() {
            Ok(frame) => {
                received += 1;
                if received <= 5 || received % 30 == 0 {
                    println!("Received frame {}: {}x{} ({} bytes)", 
                        received, frame.width, frame.height, frame.data.len());
                }
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                eprintln!("Channel disconnected!");
                break;
            }
        }
    }
    
    // Stop receiver
    running.store(false, Ordering::SeqCst);
    let _ = handle.join();
    
    println!("\n=== Summary ===");
    println!("Total frames received: {}", received);
    println!("Duration: {:.1}s", start.elapsed().as_secs_f32());
    if received > 0 {
        println!("Average FPS: {:.1}", received as f32 / start.elapsed().as_secs_f32());
    }
}

/// Frame structure matching rustjay_waaaves
struct TestFrame {
    width: u32,
    height: u32,
    data: Vec<u8>, // RGBA
    timestamp: Instant,
}

fn receive_thread(
    server_name: String,
    tx: crossbeam::channel::Sender<TestFrame>,
    running: Arc<AtomicBool>,
) {
    use objc::rc::autoreleasepool;
    
    println!("[Receiver Thread] Starting...");
    
    autoreleasepool(|| {
        let client = match syphon_core::SyphonClient::connect(&server_name) {
            Ok(c) => {
                println!("[Receiver Thread] Connected");
                c
            }
            Err(e) => {
                eprintln!("[Receiver Thread] Failed to connect: {}", e);
                return;
            }
        };
        
        let mut frame_count = 0;
        
        while running.load(Ordering::SeqCst) {
            match client.try_receive() {
                Ok(Some(mut frame)) => {
                    frame_count += 1;
                    
                    // Convert frame to RGBA (like rustjay_waaaves does)
                    match frame.to_vec() {
                        Ok(bgra_data) => {
                            let rgba_data = convert_bgra_to_rgba(&bgra_data, frame.width, frame.height);
                            
                            let test_frame = TestFrame {
                                width: frame.width,
                                height: frame.height,
                                data: rgba_data,
                                timestamp: Instant::now(),
                            };
                            
                            if tx.try_send(test_frame).is_err() {
                                println!("[Receiver Thread] Frame dropped - queue full");
                            }
                        }
                        Err(e) => {
                            eprintln!("[Receiver Thread] Failed to read frame {}: {}", frame_count, e);
                        }
                    }
                }
                Ok(None) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => {
                    eprintln!("[Receiver Thread] Error: {}", e);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        
        println!("[Receiver Thread] Stopped after {} frames", frame_count);
    });
}

/// BGRA to RGBA conversion (from rustjay_waaaves)
fn convert_bgra_to_rgba(bgra_data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut rgba_data = vec![0u8; pixel_count * 4];
    
    let actual_stride = if height > 0 {
        bgra_data.len() / height as usize
    } else {
        width as usize * 4
    };
    
    for y in 0..height as usize {
        for x in 0..width as usize {
            let src_idx = y * actual_stride + x * 4;
            let dst_idx = (y * width as usize + x) * 4;
            
            if src_idx + 3 < bgra_data.len() && dst_idx + 3 < rgba_data.len() {
                rgba_data[dst_idx] = bgra_data[src_idx + 2];     // R <- B
                rgba_data[dst_idx + 1] = bgra_data[src_idx + 1]; // G <- G
                rgba_data[dst_idx + 2] = bgra_data[src_idx];     // B <- R
                rgba_data[dst_idx + 3] = bgra_data[src_idx + 3]; // A <- A
            }
        }
    }
    
    rgba_data
}
