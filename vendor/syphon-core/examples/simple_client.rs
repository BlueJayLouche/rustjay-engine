//! Simple Syphon Client Example
//!
//! This example connects to a Syphon server and prints frame information.
//! Useful for testing basic Syphon functionality without the full application.
//!
//! Usage:
//!   cargo run --example simple_client -- "Server Name"
//!
//! If no server name is provided, it will list available servers and exit.

use std::time::{Duration, Instant};
use std::thread;

fn main() {
    // Initialize logging
    env_logger::init();
    
    println!("=== Syphon Simple Client Example ===\n");
    
    // Check if Syphon is available
    if !syphon_core::is_available() {
        eprintln!("Error: Syphon framework is not available on this system.");
        eprintln!("Make sure Syphon.framework is installed at /Library/Frameworks/");
        std::process::exit(1);
    }
    
    println!("✓ Syphon framework is available");
    
    // Get server name from command line or discover servers
    let server_name = match std::env::args().nth(1) {
        Some(name) => name,
        None => {
            println!("\nNo server name provided. Discovering available servers...\n");
            
            let servers = syphon_core::SyphonServerDirectory::servers();
            
            if servers.is_empty() {
                eprintln!("No Syphon servers found.");
                eprintln!("Make sure a Syphon server is running (e.g., Simple Server, Resolume, etc.)");
                std::process::exit(1);
            }
            
            println!("Found {} server(s):", servers.len());
            for (i, server) in servers.iter().enumerate() {
                println!("  {}. {} (app: {})", i + 1, 
                    if server.name.is_empty() { "<empty>" } else { &server.name },
                    server.app_name);
            }
            
            println!("\nUsage: cargo run --example simple_client -- \"Server Name\"");
            std::process::exit(0);
        }
    };
    
    println!("\nConnecting to server: '{}'...", server_name);
    
    // Create and connect client
    let mut client = match syphon_core::SyphonClient::connect(&server_name) {
        Ok(c) => {
            println!("✓ Connected to server");
            println!("  Server name: {}", c.server_name());
            println!("  App name: {}", c.server_app());
            c
        }
        Err(e) => {
            eprintln!("✗ Failed to connect: {}", e);
            std::process::exit(1);
        }
    };
    
    println!("\nReceiving frames for 10 seconds...\n");
    
    let start_time = Instant::now();
    let mut frame_count = 0u64;
    let mut last_fps_time = Instant::now();
    
    while start_time.elapsed() < Duration::from_secs(10) {
        match client.try_receive() {
            Ok(Some(mut frame)) => {
                frame_count += 1;
                
                // Try to get frame data
                match frame.to_vec() {
                    Ok(data) => {
                        println!("Frame {}: {}x{} ({} bytes)", 
                            frame_count, frame.width, frame.height, data.len());
                    }
                    Err(e) => {
                        eprintln!("Frame {}: {}x{} - ERROR: {}", 
                            frame_count, frame.width, frame.height, e);
                    }
                }
            }
            Ok(None) => {
                // No new frame, sleep briefly
                thread::sleep(Duration::from_millis(1));
            }
            Err(e) => {
                eprintln!("Error receiving frame: {}", e);
                thread::sleep(Duration::from_millis(100));
            }
        }
        
        // Print FPS every second
        if last_fps_time.elapsed() >= Duration::from_secs(1) {
            let fps = frame_count as f32 / start_time.elapsed().as_secs_f32();
            println!("  [FPS: {:.1}, total frames: {}]", fps, frame_count);
            last_fps_time = Instant::now();
        }
    }
    
    let fps = frame_count as f32 / start_time.elapsed().as_secs_f32();
    println!("\n=== Summary ===");
    println!("Total frames: {}", frame_count);
    println!("Average FPS: {:.1}", fps);
    println!("Duration: {:.1}s", start_time.elapsed().as_secs_f32());
    
    // Cleanup
    client.stop();
    println!("\nDisconnected.");
}
