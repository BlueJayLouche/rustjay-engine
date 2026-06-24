//! Simple example: play an audio file through the QPlayer audio engine.
//!
//! Usage:
//!   export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:$PKG_CONFIG_PATH"
//!   export FFMPEG_DIR=/opt/homebrew/Cellar/ffmpeg-full/8.0.1_3
//!   cargo run --example play_file -- /path/to/audio.wav

use qplayer_audio::SampleProvider;
use std::env;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let path = env::args()
        .nth(1)
        .expect("Usage: play_file <path-to-audio>");

    println!("QPlayer Audio Smoke Test");
    println!("========================");
    println!("File: {}", path);

    // Create audio engine
    let engine = qplayer_audio::AudioEngine::new_default()?;
    println!("Audio engine started: {} Hz, {} ch", engine.sample_rate(), engine.channels());

    // Open file
    let decoder = qplayer_audio::FileDecoder::open(&path)?;
    println!(
        "Decoder: {} Hz, {} ch, length: {:?} samples",
        decoder.sample_rate(),
        decoder.channels(),
        decoder.length()
    );

    // Play
    let _input = engine.play(Box::new(decoder));
    println!("Playing for 5 seconds... (press Ctrl+C to stop early)");

    // Refresh mixer snapshot so the audio callback sees the new input
    engine.refresh();

    // Play for 5 seconds
    thread::sleep(Duration::from_secs(5));

    println!("Done.");
    Ok(())
}
