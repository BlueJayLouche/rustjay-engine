//! Disk recorder — encode frames to video files via ffmpeg subprocess.
//!
//! Spawns an ffmpeg process and pipes raw BGRA frames to its stdin.
//! This avoids linking against encoder libraries and gives us access to
//! every codec ffmpeg supports (H.264, H.265, AV1, ProRes, etc.).
//!
//! HAP Q encode is handled separately via the local `hap-rs` workspace.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Target codec for the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderCodec {
    /// H.264 / AVC (libx264).
    H264,
    /// H.265 / HEVC (libx265).
    H265,
    /// AV1 (libsvtav1).
    AV1,
    /// Apple ProRes 422 (prores_ks).
    ProRes422,
}

impl RecorderCodec {
    /// File extension for this codec.
    pub fn extension(&self) -> &'static str {
        match self {
            RecorderCodec::H264 => "mp4",
            RecorderCodec::H265 => "mp4",
            RecorderCodec::AV1 => "mp4",
            RecorderCodec::ProRes422 => "mov",
        }
    }

    fn ffmpeg_args(&self) -> Vec<&'static str> {
        match self {
            RecorderCodec::H264 => vec![
                "-c:v", "libx264",
                "-preset", "fast",
                "-crf", "23",
                "-pix_fmt", "yuv420p",
                "-movflags", "+faststart",
            ],
            RecorderCodec::H265 => vec![
                "-c:v", "libx265",
                "-preset", "fast",
                "-crf", "28",
                "-pix_fmt", "yuv420p",
                "-movflags", "+faststart",
            ],
            RecorderCodec::AV1 => vec![
                "-c:v", "libsvtav1",
                "-preset", "8",
                "-crf", "30",
                "-pix_fmt", "yuv420p",
                "-movflags", "+faststart",
            ],
            RecorderCodec::ProRes422 => vec![
                "-c:v", "prores_ks",
                "-profile:v", "2", // 0=Proxy,1=LT,2=Normal,3=HQ
                "-pix_fmt", "yuv422p10le",
            ],
        }
    }
}

/// Active disk recorder.
pub struct Recorder {
    /// ffmpeg child process.
    child: Option<Child>,
    /// Pipe into ffmpeg stdin.
    stdin: Option<ChildStdin>,
    width: u32,
    height: u32,
    _fps: f32,
    frame_count: u64,
    /// Live audio recording alongside, muxed in on [`Recorder::finish`].
    audio: Option<AudioCapture>,
    /// Where the video encoder writes — a sidecar when there's audio to mux.
    video_path: std::path::PathBuf,
    /// Where the finished recording belongs.
    final_path: std::path::PathBuf,
}

impl Recorder {
    /// Start recording to `path`.
    ///
    /// Overwrites existing files.
    /// `audio_device` captures live audio into the same file: an AVFoundation
    /// index on macOS, a PulseAudio source on Linux, a DirectShow device name
    /// on Windows (see [`list_audio_devices`]).
    pub fn start(
        path: &Path,
        width: u32,
        height: u32,
        fps: f32,
        codec: RecorderCodec,
        audio_device: Option<&str>,
    ) -> anyhow::Result<Self> {
        // ProRes has no mp4 tag: the mp4 muxer takes the args, writes nothing and
        // leaves a 0-byte file behind. Steer it into a container that holds it.
        let mov;
        let path = if codec == RecorderCodec::ProRes422
            && path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
                != Some("mov".into())
        {
            mov = path.with_extension("mov");
            log::warn!(
                "[Recorder] {} can't hold ProRes — recording to {} instead",
                path.display(),
                mov.display()
            );
            mov.as_path()
        } else {
            path
        };

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y") // overwrite
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("bgra")
            .arg("-s")
            .arg(format!("{}x{}", width, height))
            // Stamp frames with their arrival time (no input `-r`, which would
            // override it) and conform to CFR on the way out: if the app renders
            // below `fps`, ffmpeg pads instead of letting the video timeline run
            // short of real time and drift away from the real-time audio.
            .arg("-use_wallclock_as_timestamps")
            .arg("1")
            .arg("-i")
            .arg("-"); // stdin

        // Audio records to its own file and is muxed in at the end — see
        // [`AudioCapture`] for why it can't just be this encoder's second input.
        let audio = match audio_device {
            Some(dev) => match AudioCapture::start(dev, path) {
                Ok(a) => Some(a),
                Err(e) => {
                    log::error!("[Recorder] audio capture failed ({e}) — recording silent");
                    None
                }
            },
            None => None,
        };

        // With audio, the encoder writes a sidecar the mux pass consumes.
        let video_path = match audio {
            Some(_) => sidecar(path, "video", codec.extension()),
            None => path.to_path_buf(),
        };

        cmd.args(codec.ffmpeg_args());
        cmd.args(["-fps_mode", "cfr", "-r"])
            .arg(format!("{}", fps))
            .arg("-an");

        cmd.arg(&video_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to open ffmpeg stdin"))?;

        log_stderr(&mut child, "ffmpeg");

        log::info!(
            "[Recorder] started {} {}x{} @ {:.2} fps{} → {}",
            codec.extension(),
            width,
            height,
            fps,
            if audio.is_some() { " + audio" } else { "" },
            path.display()
        );

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            width,
            height,
            _fps: fps,
            frame_count: 0,
            audio,
            video_path,
            final_path: path.to_path_buf(),
        })
    }

    /// Encode one BGRA frame.
    ///
    /// `data` must be `width * height * 4` bytes in BGRA order.
    /// Returns `false` if the ffmpeg pipe has closed.
    pub fn encode_frame(&mut self, data: &[u8]) -> bool {
        if data.len() != (self.width * self.height * 4) as usize {
            log::warn!(
                "[Recorder] frame size mismatch: expected {}, got {}",
                self.width * self.height * 4,
                data.len()
            );
            return false;
        }
        if let Some(ref mut stdin) = self.stdin {
            if stdin.write_all(data).is_err() {
                log::warn!("[Recorder] ffmpeg stdin closed");
                return false;
            }
        } else {
            return false;
        }
        self.frame_count += 1;
        true
    }

    /// Finish encoding and wait for ffmpeg to exit.
    ///
    /// With audio, the two sidecars are muxed into the final file by a detached
    /// ffmpeg — a stream copy, but a long ProRes take is many GB, so it runs in
    /// the background and logs when the file is ready.
    pub fn finish(mut self) -> anyhow::Result<()> {
        // Stop both sources back to back so their tails line up — `mux` trims
        // the audio's head by however much longer it ran.
        let mut audio = self.audio.take();
        if let Some(ref mut a) = audio {
            a.stop();
        }
        drop(self.stdin.take());
        let status = self.child.take().unwrap().wait()?;
        let audio_path = audio.and_then(AudioCapture::wait);
        if !status.success() {
            return Err(anyhow::anyhow!("ffmpeg exited with status: {}", status));
        }
        if self.frame_count == 0 {
            log::warn!(
                "[Recorder] finished with 0 frames — nothing was rendered to the \
                 output while recording, so the file is empty"
            );
        }
        log::info!("[Recorder] finished — {} frames encoded", self.frame_count);

        match audio_path {
            Some(audio) => mux(&self.video_path, &audio, &self.final_path),
            None => Ok(()),
        }
    }
}

/// Length of a media file in seconds, or 0 if ffprobe can't say.
fn duration(path: &Path) -> f64 {
    Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0.0)
}

/// `<stem>.<tag>.<ext>` beside `path` — the temporary halves of a recording.
fn sidecar(path: &Path, tag: &str, ext: &str) -> std::path::PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    path.with_file_name(format!("{stem}.{tag}.{ext}"))
}

/// Combine the video and audio sidecars into `out`, then delete them.
///
/// Detached: a stream copy still rewrites every byte, which is minutes for a
/// long ProRes take, and the caller is on the render thread.
fn mux(video: &Path, audio: &Path, out: &Path) -> anyhow::Result<()> {
    // Capture opens before the first frame arrives, so the audio runs long at
    // the head — both were stopped together, so the excess is exactly the lead.
    let lead = duration(audio) - duration(video);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    if lead > 0.02 {
        log::info!("[Recorder] trimming {:.2}s of audio lead-in", lead);
        cmd.arg("-ss").arg(format!("{lead:.3}"));
    }
    let mut child = cmd
        .arg("-i")
        .arg(audio)
        .arg("-i")
        .arg(video)
        // Audio is input #0 so `-ss` applies to it; keep video as stream #0 out.
        .args(["-map", "1:v", "-map", "0:a", "-c", "copy"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    log_stderr(&mut child, "ffmpeg-mux");

    let (video, audio, out) = (video.to_path_buf(), audio.to_path_buf(), out.to_path_buf());
    log::info!("[Recorder] muxing audio + video → {}", out.display());
    std::thread::spawn(move || match child.wait() {
        Ok(s) if s.success() => {
            let _ = std::fs::remove_file(&video);
            let _ = std::fs::remove_file(&audio);
            log::info!("[Recorder] {} is ready", out.display());
        }
        // Keep the halves on failure — they're the recording.
        Ok(s) => log::error!(
            "[Recorder] mux failed ({s}); the takes are still at {} and {}",
            video.display(),
            audio.display()
        ),
        Err(e) => log::error!("[Recorder] mux wait failed: {e}"),
    });
    Ok(())
}

/// Pipe a child's stderr into the log — ffmpeg's own errors are the only clue
/// when a recording comes out empty.
fn log_stderr(child: &mut Child, tag: &'static str) {
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                log::warn!("[{}] {}", tag, line);
            }
        });
    }
}

/// Live audio capture: cpal reads the device, a sidecar ffmpeg encodes the PCM,
/// and [`mux`] folds it into the video on stop.
///
/// Two things forced this shape. It can't be the video encoder's second input:
/// ffmpeg keeps its inputs in timestamp sync, and a capture device's clock (mach
/// uptime on macOS) starts tens of thousands of seconds ahead of the frame
/// pipe's, so it throttles the device reads to nothing. And the capture can't be
/// ffmpeg's either — its avfoundation input drops roughly 11% of samples on
/// every device here (10s of wall clock comes back as 8.9s of audio), which is
/// what makes a recording sound fast and chopped up. cpal loses 0.1%.
struct AudioCapture {
    /// ffmpeg encoding PCM from its stdin into the sidecar file.
    child: Child,
    /// Set to stop the capture thread, which drops the cpal stream.
    stop: Arc<AtomicBool>,
    /// Capture and writer threads, joined on [`Self::wait`].
    threads: Vec<std::thread::JoinHandle<()>>,
    path: std::path::PathBuf,
}

impl AudioCapture {
    fn start(device: &str, near: &Path) -> anyhow::Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let dev = host
            .input_devices()?
            .find(|d| d.description().map(|x| x.name() == device).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("audio device '{device}' not found"))?;
        let config = dev.default_input_config()?;
        let (rate, channels, format) =
            (config.sample_rate(), config.channels(), config.sample_format());

        let path = sidecar(near, "audio", "m4a");
        let mut child = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "f32le", "-ar"])
            .arg(rate.to_string())
            .arg("-ac")
            .arg(channels.to_string())
            .args(["-i", "-", "-c:a", "aac", "-b:a", "192k"])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        log_stderr(&mut child, "ffmpeg-audio");
        let mut stdin = child.stdin.take();

        // Bounded: the cpal callback must never block, so a stalled encoder
        // costs samples rather than the audio thread.
        let (tx, rx) = crossbeam::channel::bounded::<Vec<f32>>(64);
        let stop = Arc::new(AtomicBool::new(false));

        // cpal streams aren't Send, so the stream lives and dies on its own thread.
        let capture_stop = Arc::clone(&stop);
        let capture = std::thread::spawn(move || {
            use cpal::traits::StreamTrait;
            let err = |e| log::error!("[Recorder] audio stream error: {e}");
            let cfg = &config.clone().into();
            let stream = match format {
                cpal::SampleFormat::F32 => {
                    let tx = tx.clone();
                    dev.build_input_stream(cfg, move |d: &[f32], _: &_| send(&tx, d.to_vec()), err, None)
                }
                cpal::SampleFormat::I16 => {
                    let tx = tx.clone();
                    dev.build_input_stream(
                        cfg,
                        move |d: &[i16], _: &_| {
                            send(&tx, d.iter().map(|s| *s as f32 / 32768.0).collect())
                        },
                        err,
                        None,
                    )
                }
                other => {
                    log::error!("[Recorder] unsupported audio sample format {other:?}");
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    log::error!("[Recorder] cannot open audio stream: {e}");
                    return;
                }
            };
            if let Err(e) = stream.play() {
                log::error!("[Recorder] cannot start audio stream: {e}");
                return;
            }
            while !capture_stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(stream);
        });

        // Dropping stdin at the end of this thread is what makes ffmpeg finalise.
        let writer = std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                if let Some(ref mut w) = stdin
                    && w.write_all(bytemuck::cast_slice(&buf)).is_err()
                {
                    log::warn!("[Recorder] audio encoder pipe closed");
                    break;
                }
            }
            drop(stdin.take());
        });

        log::info!("[Recorder] capturing audio from '{device}' ({rate} Hz, {channels} ch)");
        Ok(Self { child, stop, threads: vec![capture, writer], path })
    }

    /// Stop capturing. Paired with [`Self::wait`] so the video pipe can close at
    /// the same moment — their tails are what the trim aligns to.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Wait for the file to be finalised, returning it if it holds anything.
    fn wait(mut self) -> Option<std::path::PathBuf> {
        self.stop();
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
        if let Err(e) = self.child.wait() {
            log::warn!("[Recorder] audio encoder wait failed: {e}");
        }
        match std::fs::metadata(&self.path) {
            Ok(m) if m.len() > 0 => Some(std::mem::take(&mut self.path)),
            _ => {
                log::warn!("[Recorder] audio capture produced nothing");
                None
            }
        }
    }
}

/// Hand a callback's samples to the writer, dropping them if it has fallen
/// behind — blocking here would stall the audio thread.
fn send(tx: &crossbeam::channel::Sender<Vec<f32>>, buf: Vec<f32>) {
    if tx.try_send(buf).is_err() {
        log::warn!("[Recorder] audio buffer overrun — encoder is behind");
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        // Only reached when `wait` wasn't called (an aborted recording).
        self.stop();
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Audio capture devices, as `(device, label)` where `device` is what
/// [`Recorder::start`] takes as `audio_device` — the same names the engine's
/// audio modulation lists.
pub fn list_audio_devices() -> Vec<(String, String)> {
    use cpal::traits::{DeviceTrait, HostTrait};
    match cpal::default_host().input_devices() {
        Ok(devices) => devices
            .filter_map(|d| d.description().ok().map(|x| x.name().to_string()))
            .map(|name| (name.clone(), name))
            .collect(),
        Err(e) => {
            log::warn!("[Recorder] cannot list audio devices: {e}");
            Vec::new()
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // If not explicitly finished, kill ffmpeg to avoid zombies.
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
    }
}
