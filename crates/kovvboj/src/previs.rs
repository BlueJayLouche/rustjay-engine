//! Library pre-visualisation: a thumbnail and a weight for every shader.
//!
//! Analysis is deliberately *not* automatic. Rendering an unknown shader costs
//! GPU time and can wedge the device, so it happens only when the user asks —
//! a "Scan Library" action run before a show, the way a DJ analyses a crate for
//! BPM. The one exception is a shader already live in a layer: hot-reload is
//! rendering it anyway, so refreshing its thumbnail there is free.
//!
//! Results live beside the engine's own config rather than in any workspace,
//! keyed by a hash of the shader source rather than by path. A shader's weight
//! is a property of the shader and this machine's GPU, not of the show you
//! happened to be building when you measured it — analysis that vanished when
//! you opened a new set would mean rescanning a whole library per gig. Hashing
//! the source means renaming or moving a shader keeps its analysis, and two
//! copies share one entry. Each record is written as it completes, so a scan
//! that dies partway costs only the shader it was on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Frames rendered before timing starts, so pipeline warm-up is excluded.
const WARMUP: u32 = 8;
/// Frames averaged for the weight.
const TIMED_FRAMES: u32 = 30;
/// Candidate frames captured for the thumbnail; the middle one by brightness wins.
const THUMB_CANDIDATES: usize = 3;
/// Stored thumbnail size. Chips are small; this is for the hover preview.
const THUMB_W: u32 = 320;
const THUMB_H: u32 = 180;

/// Whether a shader compiled, and why not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Ok,
    /// Compilation failed. The message is shown on the chip's tooltip.
    Failed(String),
}

/// What a shader needs to render: an input image, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Generator,
    Effect,
}

/// One analysed shader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub hash: String,
    pub status: Status,
    pub kind: Kind,
    /// Mean frame time in ms. `None` when the shader failed, or when only a
    /// thumbnail was captured (hot-reload never overwrites a scanned weight).
    pub ms: Option<f32>,
    /// Resolution the weight was measured at. A weight measured at a different
    /// internal resolution is meaningless, so changing it invalidates these.
    pub measured_at: Option<[u32; 2]>,
    /// Whether a thumbnail file sits beside this record.
    pub has_thumb: bool,
}

impl Record {
    /// Share of one frame at `fps` this shader costs, or `None` if unmeasured.
    ///
    /// This is what colours the chip: not a percentile across the library, but
    /// how much of the budget it actually eats. Most shaders cost almost
    /// nothing, and a scale that says so is more useful than one that spreads
    /// the collection evenly across a rainbow.
    pub fn budget_share(&self, fps: f32) -> Option<f32> {
        let ms = self.ms?;
        if fps <= 0.0 {
            return None;
        }
        Some(ms / (1000.0 / fps))
    }
}

/// Colour band for a chip, from its share of the frame budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Under 5% — stack twenty of these.
    Free,
    /// 5-12%.
    Light,
    /// 12-25%.
    Notable,
    /// 25-50%.
    Heavy,
    /// Over half a frame on its own.
    Extreme,
    /// Not analysed yet.
    Unknown,
    /// Does not compile.
    Broken,
}

impl Band {
    pub fn of(record: Option<&Record>, fps: f32) -> Band {
        let Some(r) = record else {
            return Band::Unknown;
        };
        if matches!(r.status, Status::Failed(_)) {
            return Band::Broken;
        }
        match r.budget_share(fps) {
            None => Band::Unknown,
            Some(s) if s < 0.05 => Band::Free,
            Some(s) if s < 0.12 => Band::Light,
            Some(s) if s < 0.25 => Band::Notable,
            Some(s) if s < 0.50 => Band::Heavy,
            Some(_) => Band::Extreme,
        }
    }
}

/// FNV-1a over the shader source.
///
/// ponytail: FNV rather than a real digest — this keys a local cache, it is not
/// a security boundary, and it saves a dependency. Sixty-four bits over a few
/// thousand shaders makes a collision far less likely than the disk losing the
/// file. Upgrade path: swap in a `sha2` digest if the cache ever goes public.
pub fn hash_source(src: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in src.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Does this shader consume an input image?
///
/// The ISF header is the source of truth; a bare mention of `inputImage` is the
/// fallback for headers that declare inputs without typing them.
pub fn classify(src: &str) -> Kind {
    if let Ok(isf) = rustjay_isf::header::parse(src) {
        let has_image = isf.inputs.iter().any(|i| {
            matches!(
                i.ty,
                isf::InputType::Image | isf::InputType::Audio(_) | isf::InputType::AudioFft(_)
            )
        });
        if has_image {
            return Kind::Effect;
        }
    }
    if src.contains("inputImage") {
        Kind::Effect
    } else {
        Kind::Generator
    }
}

/// The analysed-shader cache: records in memory, files in the workspace.
#[derive(Debug, Default)]
pub struct Previs {
    dir: PathBuf,
    records: HashMap<String, Record>,
}

/// Where analysis lives: global, beside the engine's other cross-set state.
///
/// Falls back to the workspace only when there is no home directory to speak
/// of, which is better than losing the cache entirely.
fn global_dir(workspace_dir: &Path) -> PathBuf {
    // Data rather than config: this is a regenerable cache that happens to be
    // expensive to regenerate, and on Linux that belongs under ~/.local/share.
    dirs::data_dir()
        .map(|d| d.join("rustjay").join("previs"))
        .unwrap_or_else(|| workspace_dir.join("previs"))
}

impl Previs {
    /// Open (and create) the cache directory, loading whatever is already there.
    ///
    /// `workspace_dir` is only consulted to migrate a cache written by an
    /// earlier build that stored analysis per-workspace, and as a last-resort
    /// location when there is no home directory.
    pub fn open(workspace_dir: &Path) -> Self {
        let dir = global_dir(workspace_dir);
        let legacy = workspace_dir.join("previs");
        if legacy != dir && legacy.is_dir() {
            migrate(&legacy, &dir);
        }
        Self::open_at(dir)
    }

    /// Open a cache at an explicit directory.
    ///
    /// Separate from [`Self::open`] so a test can point at a temporary
    /// directory: writing through the global path would have every test run
    /// scribble on the user's real analysis.
    pub fn open_at(dir: PathBuf) -> Self {
        let mut records = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&p)
                    && let Ok(r) = serde_json::from_str::<Record>(&text)
                {
                    records.insert(r.hash.clone(), r);
                }
            }
        }
        Self { dir, records }
    }

    /// The directory analysis is being read from and written to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get(&self, hash: &str) -> Option<&Record> {
        self.records.get(hash)
    }

    /// The record for a shader file, if it has been analysed.
    pub fn for_source(&self, src: &str) -> Option<&Record> {
        self.records.get(&hash_source(src))
    }

    pub fn thumb_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.png"))
    }

    fn record_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.json"))
    }

    /// Store one record, writing it through immediately.
    ///
    /// Written per shader rather than batched at the end: a scan that dies —
    /// and rendering unknown shaders is exactly the thing that might — should
    /// cost only the shader it was on, not the whole run.
    pub fn insert(&mut self, record: Record) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(
            self.record_path(&record.hash),
            serde_json::to_string_pretty(&record)?,
        )?;
        self.records.insert(record.hash.clone(), record);
        Ok(())
    }

    /// Save a thumbnail for `hash` and mark the record as having one.
    pub fn put_thumb(&mut self, hash: &str, image: &image::RgbaImage) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        image::imageops::thumbnail(image, THUMB_W, THUMB_H).save(self.thumb_path(hash))?;
        if let Some(r) = self.records.get_mut(hash) {
            r.has_thumb = true;
            let r = r.clone();
            std::fs::write(self.record_path(hash), serde_json::to_string_pretty(&r)?)?;
        }
        Ok(())
    }

    /// Shaders needing analysis: never seen, or measured at another resolution.
    ///
    /// A weight measured at a different internal resolution says nothing about
    /// this one — cost scales with pixel count — so those come back for rescan
    /// even though a record exists.
    pub fn needs_scan(&self, src: &str, at: [u32; 2]) -> bool {
        match self.records.get(&hash_source(src)) {
            None => true,
            Some(r) => match (&r.status, r.measured_at) {
                (Status::Failed(_), _) => false,
                (_, Some(res)) => res != at,
                (_, None) => true,
            },
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Pick the thumbnail from candidate frames: the one that is neither the
/// brightest nor the darkest.
///
/// A single frame is a bad sample. Strobes land on black, flashes land on
/// white, and both are useless on a chip — 59 of 1447 shaders came back pure
/// black when the harness took whatever frame it stopped on. Sorting by mean
/// luminance and taking the middle rejects both extremes.
pub fn pick_representative(frames: Vec<image::RgbaImage>) -> Option<image::RgbaImage> {
    if frames.is_empty() {
        return None;
    }
    let mut scored: Vec<(f64, image::RgbaImage)> = frames
        .into_iter()
        .map(|f| (mean_luma(&f), f))
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Some(scored.swap_remove(scored.len() / 2).1)
}

fn mean_luma(img: &image::RgbaImage) -> f64 {
    let mut sum = 0u64;
    for p in img.pixels() {
        // Rec. 601 weights, integer-scaled: enough to rank frames.
        sum += u64::from(p[0]) * 299 + u64::from(p[1]) * 587 + u64::from(p[2]) * 114;
    }
    sum as f64 / (img.pixels().len() as f64 * 1000.0)
}

/// The reference image effects are previewed against.
///
/// A test card, not a photo and not a plain grid: the grid shows warps and
/// tiling, the colour patches show hue and saturation work, the grey ramp shows
/// levels and keying, and the fine-detail block shows blur and sharpen. An
/// effect has to be visible in its own thumbnail to be worth previewing, and a
/// bare grid leaves the colour effects looking blank.
///
/// Generated rather than bundled so there is no asset to ship.
pub fn test_card(width: u32, height: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(width, height);
    let w = width as f32;
    let h = height as f32;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let fx = x as f32 / w;
        let fy = y as f32 / h;

        // Bottom-left eighth: greyscale ramp, for levels and keying.
        let (mut r, mut g, mut b) = if fy > 0.78 && fx < 0.5 {
            let v = (fx * 2.0 * 255.0) as u8;
            (v, v, v)
        } else if fy > 0.78 {
            // Bottom-right: saturated colour patches, for hue work.
            let bar = ((fx - 0.5) * 12.0) as u32 % 6;
            match bar {
                0 => (230, 30, 30),
                1 => (230, 140, 20),
                2 => (230, 220, 40),
                3 => (40, 200, 80),
                4 => (40, 120, 220),
                _ => (150, 60, 200),
            }
        } else if fx > 0.72 && fy < 0.28 {
            // Top-right: single-pixel checker, so blur and sharpen show.
            let c = ((x + y) % 2) as u8;
            (c * 255, c * 255, c * 255)
        } else {
            // Field: a smooth two-axis gradient, so warps are readable.
            (
                (fx * 210.0 + 20.0) as u8,
                (fy * 180.0 + 30.0) as u8,
                ((1.0 - fx * fy) * 170.0) as u8,
            )
        };

        // Grid lines over everything: the geometric effects are the largest
        // class in the library, and a grid is what makes a warp legible.
        let cell = (width / 16).max(8);
        if x % cell == 0 || y % cell == 0 {
            r = r.saturating_add(90);
            g = g.saturating_add(90);
            b = b.saturating_add(90);
        }
        *px = image::Rgba([r, g, b, 255]);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(v: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(4, 4, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn representative_frame_rejects_both_extremes() {
        // A strobe: one black frame, one blown-out white, one usable.
        let picked = pick_representative(vec![solid(0), solid(255), solid(120)]).unwrap();
        assert_eq!(picked.get_pixel(0, 0)[0], 120);
    }

    #[test]
    fn hash_is_content_addressed() {
        assert_eq!(hash_source("void main(){}"), hash_source("void main(){}"));
        assert_ne!(hash_source("void main(){}"), hash_source("void main(){ }"));
    }

    #[test]
    fn bands_follow_budget_share_not_percentile() {
        let rec = |ms| Record {
            hash: "x".into(),
            status: Status::Ok,
            kind: Kind::Generator,
            ms: Some(ms),
            measured_at: Some([1920, 1080]),
            has_thumb: true,
        };
        // At 60fps one frame is 16.7ms.
        assert_eq!(Band::of(Some(&rec(0.5)), 60.0), Band::Free);
        assert_eq!(Band::of(Some(&rec(1.5)), 60.0), Band::Light);
        assert_eq!(Band::of(Some(&rec(3.0)), 60.0), Band::Notable);
        assert_eq!(Band::of(Some(&rec(6.0)), 60.0), Band::Heavy);
        assert_eq!(Band::of(Some(&rec(20.0)), 60.0), Band::Extreme);
        // The same shader is judged harder at 120fps.
        assert_eq!(Band::of(Some(&rec(1.5)), 120.0), Band::Notable);
    }

    #[test]
    fn broken_and_unscanned_are_distinguishable() {
        let broken = Record {
            hash: "y".into(),
            status: Status::Failed("naga: bad".into()),
            kind: Kind::Generator,
            ms: None,
            measured_at: None,
            has_thumb: false,
        };
        assert_eq!(Band::of(Some(&broken), 60.0), Band::Broken);
        assert_eq!(Band::of(None, 60.0), Band::Unknown);
    }

    /// The bug this guards: analysis was written into the workspace, so opening
    /// a new set showed a freshly scanned library as entirely unanalysed. A
    /// weight belongs to the shader and the GPU, not to the show.
    #[test]
    fn analysis_does_not_live_in_the_workspace() {
        let ws = std::env::temp_dir().join(format!("previs-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let p = Previs::open(&ws);
        // Only meaningful where a home directory exists, which is every real
        // machine; the fallback path is exercised by not asserting otherwise.
        if dirs::data_dir().is_some() {
            assert!(
                !p.dir().starts_with(&ws),
                "analysis must not be stored per-workspace, got {}",
                p.dir().display()
            );
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// A cache written by an earlier per-workspace build must not be thrown
    /// away: rescanning a library costs hours.
    #[test]
    fn a_workspace_cache_is_carried_over() {
        let ws = std::env::temp_dir().join(format!("previs-mig-{}", std::process::id()));
        let dest = std::env::temp_dir().join(format!("previs-mig-dst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&dest);
        let legacy = ws.join("previs");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("abc.json"), "{}").unwrap();
        std::fs::write(legacy.join("abc.png"), "not really a png").unwrap();
        // Pre-existing entries win: a local measurement beats a carried one.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("abc.json"), "mine").unwrap();

        migrate(&legacy, &dest);
        assert_eq!(
            std::fs::read_to_string(dest.join("abc.json")).unwrap(),
            "mine",
            "migration must not clobber an existing record"
        );
        assert!(
            dest.join("abc.png").exists(),
            "missing files are carried over"
        );
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn a_weight_from_another_resolution_is_rescanned() {
        let dir = std::env::temp_dir().join(format!("previs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = Previs::open_at(dir.clone());
        let src = "void main(){}";
        assert!(p.needs_scan(src, [1920, 1080]));
        p.insert(Record {
            hash: hash_source(src),
            status: Status::Ok,
            kind: Kind::Generator,
            ms: Some(1.0),
            measured_at: Some([1280, 720]),
            has_thumb: false,
        })
        .unwrap();
        assert!(p.needs_scan(src, [1920, 1080]), "different resolution");
        assert!(!p.needs_scan(src, [1280, 720]), "same resolution");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_shader_is_not_rescanned_forever() {
        let dir = std::env::temp_dir().join(format!("previs-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = Previs::open_at(dir.clone());
        let src = "broken";
        p.insert(Record {
            hash: hash_source(src),
            status: Status::Failed("nope".into()),
            kind: Kind::Generator,
            ms: None,
            measured_at: None,
            has_thumb: false,
        })
        .unwrap();
        assert!(!p.needs_scan(src, [1920, 1080]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_survive_reopening() {
        let dir = std::env::temp_dir().join(format!("previs-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let hash = {
            let mut p = Previs::open_at(dir.clone());
            let h = hash_source("x");
            p.insert(Record {
                hash: h.clone(),
                status: Status::Ok,
                kind: Kind::Effect,
                ms: Some(2.5),
                measured_at: Some([1920, 1080]),
                has_thumb: false,
            })
            .unwrap();
            h
        };
        let reopened = Previs::open_at(dir.clone());
        assert_eq!(reopened.get(&hash).and_then(|r| r.ms), Some(2.5));
        assert_eq!(reopened.get(&hash).map(|r| r.kind), Some(Kind::Effect));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_card_exercises_every_effect_class() {
        let card = test_card(320, 180);
        let mut greys = 0;
        let mut saturated = 0;
        for p in card.pixels() {
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            let spread = r.max(g).max(b) - r.min(g).min(b);
            if spread < 6 {
                greys += 1;
            }
            if spread > 120 {
                saturated += 1;
            }
        }
        // Both a neutral ramp (for levels/keying) and saturated colour (for hue
        // work) must exist, or the colour effects preview as a flat field.
        assert!(greys > 200, "no neutral region: {greys}");
        assert!(saturated > 200, "no saturated region: {saturated}");
        // Set PREVIS_DUMP_CARD to eyeball the card while tuning it.
        if let Ok(path) = std::env::var("PREVIS_DUMP_CARD") {
            test_card(1280, 720).save(path).unwrap();
        }
    }
}

/// Copy a per-workspace cache into the global one, once.
///
/// Only fills gaps: a record already measured on this machine is better than
/// one carried over from a workspace, and copying is skipped entirely if the
/// destination already has an entry.
fn migrate(from: &Path, to: &Path) {
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    let mut moved = 0usize;
    for e in entries.flatten() {
        let src = e.path();
        let Some(name) = src.file_name() else {
            continue;
        };
        let dst = to.join(name);
        if dst.exists() {
            continue;
        }
        if std::fs::copy(&src, &dst).is_ok() {
            moved += 1;
        }
    }
    if moved > 0 {
        log::info!(
            "[Previs] carried {moved} file(s) over from {} — analysis is global now",
            from.display()
        );
    }
}

/// Renders one shader off to the side to measure it and capture a thumbnail.
///
/// Holds its own target, readback buffer and test-card texture so a scan
/// allocates once and reuses them across the whole library.
///
/// ponytail: this runs on the app's own device rather than in a child process,
/// so a shader that wedges the GPU takes the app with it. The mitigation is
/// [`Previs::insert`] writing through per shader — a wedge costs one restart
/// and the scan resumes. Upgrade path: shell out to the `isf_bench` example if
/// scanning untrusted packs ever becomes routine.
pub struct Analyzer {
    target: rustjay_render::Texture,
    card: rustjay_render::Texture,
    readback: wgpu::Buffer,
    size: [u32; 2],
    /// Bytes per row in `readback`, padded up to wgpu's copy alignment.
    padded_bpr: u32,
}

impl Analyzer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: [u32; 2]) -> Self {
        let [w, h] = size;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = (w * 4).div_ceil(align) * align;
        let card = rustjay_render::Texture::create_render_target(device, w, h, "previs card");
        card.update(queue, &test_card(w, h));
        Self {
            target: rustjay_render::Texture::create_render_target(device, w, h, "previs target"),
            card,
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("previs readback"),
                size: u64::from(padded_bpr * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            size,
            padded_bpr,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Compile, time and photograph one shader.
    ///
    /// Returns the record plus the frame chosen for its thumbnail. A shader
    /// that fails to compile returns a `Failed` record and no image — that is a
    /// result worth caching, not an error: it is what greys the chip out.
    pub fn analyze(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quad: &wgpu::Buffer,
        path: &Path,
    ) -> anyhow::Result<(Record, Option<image::RgbaImage>)> {
        let src = std::fs::read_to_string(path)?;
        let hash = hash_source(&src);
        let kind = classify(&src);

        let mut effect = match rustjay_isf::IsfEffect::from_path(path) {
            Ok(e) => e,
            Err(e) => {
                return Ok((
                    Record {
                        hash,
                        status: Status::Failed(e.to_string()),
                        kind,
                        ms: None,
                        measured_at: None,
                        has_thumb: false,
                    },
                    None,
                ));
            }
        };
        // A pinned step keeps a scan reproducible: the same shader scanned
        // twice gets the same thumbnail rather than whatever the wall clock
        // happened to land on.
        effect.fixed_delta = Some(1.0 / 60.0);
        rustjay_core::EffectPlugin::init(&mut effect, device, queue);
        if let Some(err) = &effect.transpile_error {
            return Ok((
                Record {
                    hash,
                    status: Status::Failed(err.clone()),
                    kind,
                    ms: None,
                    measured_at: None,
                    has_thumb: false,
                },
                None,
            ));
        }

        let mut state = rustjay_core::EffectPlugin::default_state(&effect);
        let mut engine = rustjay_core::EngineState::new();
        engine.resolution.internal_width = self.size[0];
        engine.resolution.internal_height = self.size[1];

        for _ in 0..WARMUP {
            self.frame(device, queue, quad, &mut effect, &mut state, &engine);
        }

        // Capture candidates spread through the timed run, so an animated
        // shader offers genuinely different frames to choose between.
        let every = (TIMED_FRAMES / THUMB_CANDIDATES as u32).max(1);
        let mut frames = Vec::with_capacity(THUMB_CANDIDATES);
        let start = std::time::Instant::now();
        for i in 0..TIMED_FRAMES {
            self.frame(device, queue, quad, &mut effect, &mut state, &engine);
            if i % every == 0
                && frames.len() < THUMB_CANDIDATES
                && let Some(img) = self.read_back(device, queue)
            {
                frames.push(img);
            }
        }
        let ms = start.elapsed().as_secs_f32() * 1000.0 / TIMED_FRAMES as f32;

        Ok((
            Record {
                hash,
                status: Status::Ok,
                kind,
                ms: Some(ms),
                measured_at: Some(self.size),
                has_thumb: false,
            },
            pick_representative(frames),
        ))
    }

    /// One frame, blocking until the GPU has finished it so the timing means
    /// something.
    fn frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quad: &wgpu::Buffer,
        effect: &mut rustjay_isf::IsfEffect,
        state: &mut rustjay_isf::IsfState,
        engine: &rustjay_core::EngineState,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("previs"),
        });
        {
            // An effect samples the test card; a generator is handed one
            // anyway and simply ignores it.
            let input = rustjay_core::EffectInput {
                view: &self.card.view,
                sampler: &self.card.sampler,
                generation: self.card.generation,
                texture: Some(&self.card.texture),
            };
            let mut ctx = rustjay_core::RenderHookCtx {
                encoder: &mut encoder,
                device,
                queue,
                input: Some(input),
                input_b: None,
                target_view: &self.target.view,
                engine_state: engine,
                vertex_buffer: quad,
            };
            rustjay_core::EffectPlugin::render(effect, &mut ctx, state);
        }
        queue.submit(std::iter::once(encoder.finish()));
        while device
            .poll(wgpu::PollType::Poll)
            .is_ok_and(|s| !s.is_queue_empty())
        {
            std::thread::yield_now();
        }
    }

    /// Copy the current target back to the CPU as RGBA8.
    fn read_back(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<image::RgbaImage> {
        let [w, h] = self.size;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("previs read"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&done);
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                if r.is_ok() {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            });
        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            if device.poll(wgpu::PollType::Poll).is_err() {
                return None;
            }
            std::thread::yield_now();
        }

        let bgra = matches!(
            rustjay_core::working_format(),
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let img = {
            let data = self.readback.slice(..).get_mapped_range().ok()?;
            let mut out = image::RgbaImage::new(w, h);
            for y in 0..h {
                let row = (y * self.padded_bpr) as usize;
                for x in 0..w {
                    let i = row + (x * 4) as usize;
                    let p = data.get(i..i + 4)?;
                    // The working format is whatever the swapchain wants; the
                    // thumbnail is always RGBA on disk.
                    let px = if bgra {
                        [p[2], p[1], p[0], p[3]]
                    } else {
                        [p[0], p[1], p[2], p[3]]
                    };
                    out.put_pixel(x, y, image::Rgba(px));
                }
            }
            out
        };
        self.readback.unmap();
        Some(img)
    }
}

/// A library scan in flight.
///
/// One shader is analysed per frame rather than the whole queue at once: each
/// shader blocks the GPU for tens of frames, so a batch would freeze the app
/// with no progress and no way out. Per-frame keeps the readout moving and the
/// cancel button live.
#[derive(Debug, Default)]
pub struct ScanJob {
    queue: Vec<PathBuf>,
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    cancel: bool,
}

impl ScanJob {
    /// Queue every shader that needs analysing at this resolution.
    ///
    /// Anything already analysed at the same resolution is skipped, which is
    /// what makes a rescan after adding a few shaders to a large folder cheap.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>, previs: &Previs, at: [u32; 2]) -> Self {
        let queue: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| match std::fs::read_to_string(p) {
                Ok(src) => previs.needs_scan(&src, at),
                Err(_) => false,
            })
            .collect();
        Self {
            total: queue.len(),
            queue,
            done: 0,
            failed: 0,
            cancel: false,
        }
    }

    /// Re-analyse everything, ignoring what is already cached.
    pub fn rescan(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let queue: Vec<PathBuf> = paths.into_iter().collect();
        Self {
            total: queue.len(),
            queue,
            done: 0,
            failed: 0,
            cancel: false,
        }
    }

    pub fn cancel(&mut self) {
        self.cancel = true;
    }

    pub fn is_finished(&self) -> bool {
        self.cancel || self.queue.is_empty()
    }

    /// What to show in the menu bar while this runs.
    pub fn label(&self) -> String {
        match self.queue.last().and_then(|p| p.file_name()) {
            Some(name) => format!(
                "Scanning {}/{} — {}",
                self.done + 1,
                self.total,
                name.to_string_lossy()
            ),
            None => format!("Scanning {}/{}", self.done, self.total),
        }
    }

    /// Analyse the next shader. Returns false when the job is done.
    pub fn step(
        &mut self,
        analyzer: &mut Analyzer,
        previs: &mut Previs,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quad: &wgpu::Buffer,
    ) -> bool {
        if self.cancel {
            self.queue.clear();
            return false;
        }
        let Some(path) = self.queue.pop() else {
            return false;
        };
        self.done += 1;
        match analyzer.analyze(device, queue, quad, &path) {
            Ok((record, thumb)) => {
                if matches!(record.status, Status::Failed(_)) {
                    self.failed += 1;
                }
                let hash = record.hash.clone();
                if let Err(e) = previs.insert(record) {
                    log::warn!(
                        "[Previs] could not write record for {}: {e}",
                        path.display()
                    );
                }
                if let Some(img) = thumb
                    && let Err(e) = previs.put_thumb(&hash, &img)
                {
                    log::warn!("[Previs] could not write thumb for {}: {e}", path.display());
                }
            }
            Err(e) => {
                self.failed += 1;
                log::warn!("[Previs] {} could not be analysed: {e}", path.display());
            }
        }
        !self.queue.is_empty()
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn a_scan_skips_what_is_already_analysed_at_this_resolution() {
        let dir = std::env::temp_dir().join(format!("previs-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.fs");
        let b = dir.join("b.fs");
        std::fs::write(&a, "shader a").unwrap();
        std::fs::write(&b, "shader b").unwrap();

        let mut previs = Previs::open_at(dir.clone());
        previs
            .insert(Record {
                hash: hash_source("shader a"),
                status: Status::Ok,
                kind: Kind::Generator,
                ms: Some(1.0),
                measured_at: Some([1920, 1080]),
                has_thumb: true,
            })
            .unwrap();

        // `a` is cached at this resolution, so only `b` is queued.
        let job = ScanJob::new(vec![a.clone(), b.clone()], &previs, [1920, 1080]);
        assert_eq!(job.total, 1);
        // At another resolution its weight is meaningless, so both come back.
        let job = ScanJob::new(vec![a.clone(), b.clone()], &previs, [1280, 720]);
        assert_eq!(job.total, 2);
        // A forced rescan ignores the cache entirely.
        assert_eq!(ScanJob::rescan(vec![a, b]).total, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_stops_the_queue() {
        let mut job = ScanJob::rescan(vec![PathBuf::from("x.fs"), PathBuf::from("y.fs")]);
        assert!(!job.is_finished());
        job.cancel();
        assert!(job.is_finished());
    }
}

impl Band {
    /// Dot colour for a library row.
    ///
    /// Deliberately calm at the low end: 77% of a real corpus lands in `Free`,
    /// and a heatmap where almost everything is quiet makes the few loud
    /// entries findable. Colouring by percentile instead would paint a rainbow
    /// over differences of a third of a millisecond.
    pub fn colour(self) -> [u8; 3] {
        match self {
            Band::Free => [70, 140, 90],
            Band::Light => [150, 160, 70],
            Band::Notable => [200, 150, 60],
            Band::Heavy => [210, 100, 55],
            Band::Extreme => [200, 60, 60],
            Band::Unknown => [90, 90, 95],
            Band::Broken => [120, 60, 70],
        }
    }

    /// What the row's tooltip says about cost.
    pub fn describe(self, ms: Option<f32>) -> String {
        let cost = match ms {
            Some(ms) => format!("{ms:.2} ms/frame — "),
            None => String::new(),
        };
        match self {
            Band::Free => format!("{cost}under 5% of a frame; stack freely"),
            Band::Light => format!("{cost}5-12% of a frame"),
            Band::Notable => format!("{cost}12-25% of a frame"),
            Band::Heavy => format!("{cost}a quarter to half a frame"),
            Band::Extreme => format!("{cost}over half a frame on its own"),
            Band::Unknown => "Not analysed — run Library ▸ Scan library".to_string(),
            Band::Broken => "Does not compile".to_string(),
        }
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    /// Pins the hash against the values `checked/seed_previs.py` produces.
    ///
    /// The seed script reimplements FNV-1a in Python; if either side drifts,
    /// every seeded record silently resolves to nothing and the library looks
    /// unanalysed. Cheaper to catch here than to debug there.
    #[test]
    fn hash_matches_the_seed_script() {
        assert_eq!(hash_source(""), "cbf29ce484222325");
        assert_eq!(hash_source("void"), "3173c900e37ae1df");
        assert_eq!(hash_source("void main(){}"), "f82eeec12e61704f");
        assert_eq!(hash_source("a"), "af63dc4c8601ec8c");
    }

    /// A seeded record — no weight yet — must load and read as "not analysed".
    #[test]
    fn a_seeded_record_loads_and_asks_to_be_scanned() {
        let json = r#"{
            "hash": "00294b5f72e5a50e",
            "status": "Ok",
            "kind": "Generator",
            "ms": null,
            "measured_at": null,
            "has_thumb": true
        }"#;
        let r: Record = serde_json::from_str(json).expect("seeded record must deserialise");
        assert!(r.has_thumb);
        assert_eq!(r.ms, None);
        // A thumbnail without a weight still needs scanning locally.
        assert_eq!(Band::of(Some(&r), 60.0), Band::Unknown);

        let failed = r#"{
            "hash": "deadbeef",
            "status": {"Failed": "naga: bad"},
            "kind": "Effect",
            "ms": null,
            "measured_at": null,
            "has_thumb": false
        }"#;
        let f: Record = serde_json::from_str(failed).expect("failed record must deserialise");
        assert_eq!(Band::of(Some(&f), 60.0), Band::Broken);
    }
}
