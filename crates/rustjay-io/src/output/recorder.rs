//! Disk recorder — encode frames to video files via ffmpeg subprocess.
//!
//! Spawns an ffmpeg process and pipes raw BGRA frames to its stdin.
//! This avoids linking against encoder libraries and gives us access to
//! every codec ffmpeg supports (H.264, H.265, AV1, ProRes, etc.).
//!
//! HAP Q encode is handled separately via the local `hap-rs` workspace.

use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

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
}

impl Recorder {
    /// Start recording to `path`.
    ///
    /// Overwrites existing files.
    pub fn start(
        path: &Path,
        width: u32,
        height: u32,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y") // overwrite
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("bgra")
            .arg("-s")
            .arg(format!("{}x{}", width, height))
            .arg("-r")
            .arg(format!("{}", fps))
            .arg("-i")
            .arg("-") // stdin
            .args(codec.ffmpeg_args())
            .arg("-an") // no audio
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to open ffmpeg stdin"))?;

        log::info!(
            "[Recorder] started {} {}x{} @ {:.2} fps → {}",
            codec.extension(),
            width,
            height,
            fps,
            path.display()
        );

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            width,
            height,
            _fps: fps,
            frame_count: 0,
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
    pub fn finish(mut self) -> anyhow::Result<()> {
        drop(self.stdin.take());
        match self.child.take().unwrap().wait() {
            Ok(status) => {
                if status.success() {
                    log::info!(
                        "[Recorder] finished — {} frames encoded",
                        self.frame_count
                    );
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(
                        "ffmpeg exited with status: {}",
                        status
                    ))
                }
            }
            Err(e) => Err(anyhow::anyhow!("ffmpeg wait failed: {}", e)),
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
