use cuepool_core::{AudioRouting, Cue, CueBase, LoopMode, ShowFile, Timespan, TriggerMode};
use rust_decimal::Decimal;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
    dir: PathBuf,
    pub project: PathBuf,
}

impl Fixture {
    pub fn new(cues: Vec<Cue>) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cuepool-headless-{}-{nonce}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir)?;
        write_wav(&dir.join("tone.wav"), 4_800)?;
        write_y4m(&dir.join("video.y4m"), 5)?;
        let show = ShowFile {
            cues,
            ..ShowFile::default()
        };
        let project = dir.join("show.qproj");
        fs::write(
            &project,
            serde_json::to_vec_pretty(&show).map_err(io::Error::other)?,
        )?;
        Ok(Self { dir, project })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn base(qid: i64, trigger: TriggerMode) -> CueBase {
    CueBase {
        qid: Decimal::from(qid),
        name: format!("Cue {qid}"),
        trigger,
        ..CueBase::default()
    }
}

pub fn sound(qid: i64, trigger: TriggerMode) -> Cue {
    Cue::Sound {
        base: base(qid, trigger),
        path: "tone.wav".into(),
        start_time: Timespan::ZERO,
        duration: Timespan::ZERO,
        volume: 1.0,
        pan: 0.0,
        fade_in: 0.0,
        fade_out: 0.0,
        fade_type: Default::default(),
        eq: None,
        routing: AudioRouting::default(),
    }
}

pub fn dummy(qid: i64, trigger: TriggerMode) -> Cue {
    Cue::Dummy {
        base: base(qid, trigger),
    }
}

pub fn video(qid: i64, loop_mode: LoopMode) -> Cue {
    let mut base = base(qid, TriggerMode::Go);
    base.loop_mode = loop_mode;
    Cue::Video {
        base,
        path: "video.y4m".into(),
        start_time: Timespan::ZERO,
        duration: Timespan::from_secs_f64(0.2),
        volume: 1.0,
        pan: 0.0,
        fade_in: 0.0,
        fade_out: 0.0,
        fade_type: Default::default(),
        eq: None,
        routing: AudioRouting::default(),
        follow_mtc: false,
        mtc_start: Timespan::ZERO,
    }
}

fn write_wav(path: &Path, frames: u32) -> io::Result<()> {
    let channels = 2u16;
    let sample_rate = 48_000u32;
    let bits = 16u16;
    let data_len = frames * u32::from(channels) * u32::from(bits / 8);
    let mut file = fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * u32::from(channels) * 2).to_le_bytes())?;
    file.write_all(&(channels * 2).to_le_bytes())?;
    file.write_all(&bits.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for frame in 0..frames {
        let sample = (((frame % 200) as i32 - 100) * 200) as i16;
        file.write_all(&sample.to_le_bytes())?;
        file.write_all(&(-sample).to_le_bytes())?;
    }
    Ok(())
}

fn write_y4m(path: &Path, frames: u8) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(b"YUV4MPEG2 W4 H4 F25:1 Ip A1:1 C420jpeg\n")?;
    for frame in 0..frames {
        file.write_all(b"FRAME\n")?;
        file.write_all(&[32 + frame * 20; 16])?;
        file.write_all(&[96 + frame; 4])?;
        file.write_all(&[160 - frame; 4])?;
    }
    Ok(())
}
