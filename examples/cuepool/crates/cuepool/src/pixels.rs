//! Pixel-map sample feed — a WebSocket mirror of the canvas samples for an
//! external visualiser.
//!
//! The samples already exist: `main.rs` publishes every pixel-sampler readback
//! into `SharedState::lighting_preview` for the lighting panel's grid preview,
//! and this module is a second reader on that same state. Nothing in the
//! sampler, the render path, or the lighting engine changes.
//!
//! # Why a separate listener from the show-control API
//!
//! Pixel data is the lowest-value thing CuePool holds — it is what the room can
//! already see on the projectors. `/v1/project` and `/v1/logs` are the highest.
//! Sharing one port would force pixels to be protected at the level show files
//! need, or show files at the level pixels need. One port each keeps the API's
//! loopback-only guard (`api::validate_bind_address`) untouched and lets a show
//! network firewall permit the visualiser and deny the API.
//!
//! # Why no authentication
//!
//! sACN and Art-Net carry none, so CuePool's real DMX output is already on this
//! network in clear, where anyone can drive the rig outright. Gating a
//! read-only mirror of that behind a secret is inconsistent, and a secret to
//! distribute is operational cost at curtain-up. The realistic failure here is
//! a reloaded browser tab, not an attacker, so the control is
//! [`MAX_CONNECTIONS`] — bounding how many pollers can contend the shared state
//! lock — rather than a token. Operators wanting narrower reach should bind a
//! specific interface rather than `0.0.0.0`.

use axum::Router;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use cuepool_core::lighting::{PixelMapSegment, SegmentSource};
use cuepool_gui::SharedStateHandle;
use rustjay_lighting::ScanOrder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Environment variable naming the bind address. Unset means the feed never
/// starts — no new port on a machine that did not ask for one.
const BIND_VAR: &str = "CUEPOOL_PIXELS_BIND";

/// Concurrent visualiser connections. Each one polls `SharedState` on its own
/// interval, and that lock is also taken by the sampler and the GUI, so this is
/// a load-shedding bound rather than a licensing one.
const MAX_CONNECTIONS: usize = 8;

/// Client frames are not part of the protocol; cap reads so a misbehaving one
/// cannot buffer without limit.
const MAX_CLIENT_FRAME: usize = 1024;

const DEFAULT_FPS: f32 = 30.0;
const MIN_FPS: f32 = 1.0;
const MAX_FPS: f32 = 60.0;

/// Bytes preceding the RGBA payload of a pixel frame. See [`encode_frame`].
pub(crate) const FRAME_HEADER_BYTES: usize = 12;

/// Start the feed if [`BIND_VAR`] is set, otherwise do nothing. `Err` means the
/// operator asked for one and it could not be honoured.
///
/// There is no handle to keep: the listener is a display-only surface with no
/// results a client is owed, so it ends with the process rather than carrying
/// the API's graceful-shutdown plumbing.
pub fn start(shared: SharedStateHandle) -> anyhow::Result<()> {
    let Some(bind) = std::env::var(BIND_VAR)
        .ok()
        .filter(|b| !b.trim().is_empty())
    else {
        return Ok(());
    };
    let address: SocketAddr = bind
        .trim()
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid {BIND_VAR} '{bind}': {error}"))?;
    let listener = TcpListener::bind(address)
        .map_err(|error| anyhow::anyhow!("cannot bind CuePool pixel feed to {address}: {error}"))?;
    listener.set_nonblocking(true)?;
    let local_addr = listener.local_addr()?;

    if !local_addr.ip().is_loopback() {
        log::info!(
            "CuePool pixel feed is reachable off this machine on {local_addr}; it is unauthenticated by design"
        );
    }

    let state = FeedState {
        shared,
        connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
    };
    std::thread::Builder::new()
        .name("cuepool-pixels".into())
        .spawn(move || run(listener, state))?;

    log::info!("CuePool pixel feed on ws://{local_addr}/v1/pixels");
    Ok(())
}

fn run(listener: TcpListener, state: FeedState) {
    // Its own runtime, not the API's: that one is `new_current_thread`, and a
    // 60 Hz firehose sharing a thread with request handling would contend with
    // the status poll.
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| {
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                let router = Router::new()
                    .route("/v1/pixels", get(upgrade))
                    .with_state(state);
                axum::serve(listener, router).await?;
                Ok(())
            })
        });
    if let Err(error) = result {
        log::error!("CuePool pixel feed stopped: {error}");
    }
}

#[derive(Clone)]
struct FeedState {
    shared: SharedStateHandle,
    connections: Arc<Semaphore>,
}

#[derive(Debug, Deserialize)]
struct FeedParams {
    /// Poll rate, clamped to [`MIN_FPS`]..=[`MAX_FPS`].
    fps: Option<f32>,
    /// Comma-separated segment ids. Absent means every active segment.
    segments: Option<String>,
}

impl FeedParams {
    fn interval(&self) -> Duration {
        let fps = self.fps.unwrap_or(DEFAULT_FPS).clamp(MIN_FPS, MAX_FPS);
        Duration::from_secs_f32(1.0 / fps)
    }

    /// `None` = no filter. Unparseable ids are dropped rather than failing the
    /// upgrade: a visualiser with one stale id in its list should still render
    /// the rest.
    fn filter(&self) -> Option<Vec<u32>> {
        let raw = self.segments.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        let ids: Vec<u32> = raw
            .split(',')
            .filter_map(|id| id.trim().parse().ok())
            .collect();
        (!ids.is_empty()).then_some(ids)
    }
}

async fn upgrade(
    State(state): State<FeedState>,
    Query(params): Query<FeedParams>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Ok(permit) = Arc::clone(&state.connections).try_acquire_owned() else {
        log::warn!("CuePool pixel feed refused a connection: {MAX_CONNECTIONS} already open");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("pixel feed is at its {MAX_CONNECTIONS}-connection limit"),
        )
            .into_response();
    };
    let interval = params.interval();
    let filter = params.filter();
    upgrade
        .max_frame_size(MAX_CLIENT_FRAME)
        .on_upgrade(move |socket| async move {
            stream(socket, state.shared, interval, filter).await;
            drop(permit);
        })
}

/// One connection: metadata on change, pixel frames on change, until the client
/// goes away.
async fn stream(
    mut socket: WebSocket,
    shared: SharedStateHandle,
    interval: Duration,
    filter: Option<Vec<u32>>,
) {
    let started = Instant::now();
    let mut last_meta: Option<Vec<SegmentMeta>> = None;
    let mut last_pixels: HashMap<u32, Vec<u8>> = HashMap::new();

    let mut ticker = tokio::time::interval(interval);
    // Drop-on-lag rather than queue: awaiting a slow client's `send` below
    // delays the next tick, and Skip discards the ones missed meanwhile. A
    // visualiser wants current state, not a backlog.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Detect close/error. Clients are not expected to say anything;
            // anything they do send is drained and ignored. Cancel-safe: this
            // is `Stream::poll_next` underneath, and a dropped read costs at
            // most a client frame we would have discarded anyway.
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            },
            _ = ticker.tick() => {}
        }

        let elapsed_ms = started.elapsed().as_millis() as u32;
        // Snapshot under one lock, then release it before any await — the
        // sampler and GUI need this mutex every frame.
        let Some(snapshot) = snapshot(&shared, filter.as_deref(), &last_pixels, elapsed_ms) else {
            continue;
        };

        if last_meta.as_ref() != Some(&snapshot.meta) {
            let Ok(json) = serde_json::to_string(&MetaFrame {
                kind: "segments",
                segments: &snapshot.meta,
            }) else {
                continue;
            };
            if socket
                .send(Message::Text(Utf8Bytes::from(json)))
                .await
                .is_err()
            {
                return;
            }
            // Geometry changed, so cached payloads may be the wrong size.
            last_pixels.clear();
            last_meta = Some(snapshot.meta);
        }

        for (id, frame, rgba) in snapshot.frames {
            if socket.send(Message::Binary(frame.into())).await.is_err() {
                return;
            }
            last_pixels.insert(id, rgba);
        }
    }
}

struct Snapshot {
    meta: Vec<SegmentMeta>,
    /// `(segment id, encoded frame, raw RGBA to cache once sent)`.
    frames: Vec<(u32, Vec<u8>, Vec<u8>)>,
}

/// Read geometry and pixels under a single lock. `None` if the lock is
/// poisoned — a tick is skipped rather than killing the connection.
fn snapshot(
    shared: &SharedStateHandle,
    filter: Option<&[u32]>,
    last_pixels: &HashMap<u32, Vec<u8>>,
    elapsed_ms: u32,
) -> Option<Snapshot> {
    let state = shared.lock().ok()?;
    let wanted = |id: u32| filter.is_none_or(|ids| ids.contains(&id));

    let meta: Vec<SegmentMeta> = state
        .show_file
        .lighting
        .active_segments()
        .filter(|segment| wanted(segment.id))
        .map(SegmentMeta::from)
        .collect();

    let frames = meta
        .iter()
        .filter_map(|segment| {
            let (cols, rows, rgba) = state.lighting_preview.get(&segment.id)?;
            // Guard the cast: `cols`/`rows` are clamped to 512 by the sampler,
            // but the preview map is public state.
            let (cols, rows) = (u16::try_from(*cols).ok()?, u16::try_from(*rows).ok()?);
            if rgba.len() != cols as usize * rows as usize * 4 {
                return None; // mid-resize; the next tick will be consistent
            }
            if last_pixels
                .get(&segment.id)
                .is_some_and(|last| last == rgba)
            {
                return None; // unchanged since last send — static content is free
            }
            Some((
                segment.id,
                encode_frame(segment.id, cols, rows, elapsed_ms, rgba),
                rgba.clone(),
            ))
        })
        .collect();

    Some(Snapshot { meta, frames })
}

/// Binary pixel frame: a 12-byte little-endian header then tightly packed RGBA,
/// row-major from the top-left.
///
/// ```text
/// 0  u32  segment id
/// 4  u16  cols
/// 6  u16  rows
/// 8  u32  milliseconds since the stream opened
/// 12 ..   cols * rows * 4 bytes RGBA
/// ```
///
/// This is the sampler's native order, before `demux_tile` applies the
/// segment's wiring. Clients wanting fixture order apply `order` from the
/// metadata frame themselves; clients drawing a grid ignore it.
pub(crate) fn encode_frame(id: u32, cols: u16, rows: u16, elapsed_ms: u32, rgba: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + rgba.len());
    frame.extend_from_slice(&id.to_le_bytes());
    frame.extend_from_slice(&cols.to_le_bytes());
    frame.extend_from_slice(&rows.to_le_bytes());
    frame.extend_from_slice(&elapsed_ms.to_le_bytes());
    frame.extend_from_slice(rgba);
    frame
}

#[derive(Debug, Serialize)]
struct MetaFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    segments: &'a [SegmentMeta],
}

/// Everything a visualiser needs to lay a segment out, so it never has to reach
/// for the show-control API — which is the whole point of the separate port.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SegmentMeta {
    id: u32,
    name: String,
    cols: u32,
    rows: u32,
    region: [f32; 4],
    source: SegmentSource,
    universe: u16,
    address: u16,
    gamma: f32,
    order: ScanOrder,
}

impl From<&PixelMapSegment> for SegmentMeta {
    fn from(segment: &PixelMapSegment) -> Self {
        Self {
            id: segment.id,
            name: segment.name.clone(),
            cols: segment.cols,
            rows: segment.rows,
            region: segment.region,
            source: segment.source,
            universe: segment.universe,
            address: segment.address,
            gamma: segment.gamma,
            order: segment.order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A segment at `cols`x`rows` using the builtin `rgb` profile, so
    /// `active_segments` keeps it.
    fn segment(id: u32, cols: u32, rows: u32) -> PixelMapSegment {
        PixelMapSegment {
            cols,
            rows,
            source: SegmentSource::Canvas,
            ..PixelMapSegment::new(id)
        }
    }

    /// Shared state carrying `segments` in the patch and `preview` as the
    /// sampler's latest readbacks.
    fn state(
        segments: Vec<PixelMapSegment>,
        preview: Vec<(u32, u32, u32, Vec<u8>)>,
    ) -> SharedStateHandle {
        let mut shared = cuepool_gui::SharedState::default();
        shared.show_file.lighting.enabled = true;
        shared.show_file.lighting.segments = segments;
        for (id, cols, rows, rgba) in preview {
            shared.lighting_preview.insert(id, (cols, rows, rgba));
        }
        Arc::new(Mutex::new(shared))
    }

    #[test]
    fn a_sampled_segment_becomes_one_frame_carrying_its_own_dimensions() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![9u8; 8])]);
        let empty = HashMap::new();

        let snap = snapshot(&shared, None, &empty, 5).expect("lock held");

        assert_eq!(snap.meta.len(), 1);
        assert_eq!(snap.frames.len(), 1);
        let (id, frame, rgba) = &snap.frames[0];
        assert_eq!(*id, 7);
        assert_eq!(rgba, &vec![9u8; 8]);
        assert_eq!(frame.len(), FRAME_HEADER_BYTES + 8);
        assert_eq!(&frame[0..4], &7u32.to_le_bytes());
        assert_eq!(&frame[4..6], &2u16.to_le_bytes());
        assert_eq!(&frame[6..8], &1u16.to_le_bytes());
    }

    #[test]
    fn unchanged_pixels_send_nothing_but_still_report_geometry() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![9u8; 8])]);
        let cached = HashMap::from([(7, vec![9u8; 8])]);

        let snap = snapshot(&shared, None, &cached, 5).expect("lock held");

        assert_eq!(snap.meta.len(), 1, "geometry is still advertised");
        assert!(snap.frames.is_empty(), "static content should cost nothing");
    }

    #[test]
    fn changed_pixels_resend() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![1u8; 8])]);
        let cached = HashMap::from([(7, vec![9u8; 8])]);

        let snap = snapshot(&shared, None, &cached, 5).expect("lock held");

        assert_eq!(snap.frames.len(), 1);
    }

    #[test]
    fn the_filter_drops_segments_from_both_metadata_and_frames() {
        let shared = state(
            vec![segment(1, 2, 1), segment(2, 2, 1)],
            vec![(1, 2, 1, vec![0u8; 8]), (2, 2, 1, vec![0u8; 8])],
        );
        let empty = HashMap::new();

        let snap = snapshot(&shared, Some(&[2]), &empty, 0).expect("lock held");

        assert_eq!(snap.meta.len(), 1);
        assert_eq!(snap.meta[0].id, 2);
        assert_eq!(snap.frames.len(), 1);
        assert_eq!(snap.frames[0].0, 2);
    }

    #[test]
    fn a_payload_disagreeing_with_its_dimensions_is_skipped() {
        // The sampler is mid-resize: the tuple says 4x4 but carries 2x1 worth.
        let shared = state(vec![segment(7, 4, 4)], vec![(7, 4, 4, vec![0u8; 8])]);
        let empty = HashMap::new();

        let snap = snapshot(&shared, None, &empty, 0).expect("lock held");

        assert_eq!(snap.meta.len(), 1);
        assert!(snap.frames.is_empty(), "a short payload must not be framed");
    }

    #[test]
    fn a_patched_segment_the_sampler_has_not_reached_yet_yields_metadata_only() {
        let shared = state(vec![segment(7, 2, 1)], vec![]);
        let empty = HashMap::new();

        let snap = snapshot(&shared, None, &empty, 0).expect("lock held");

        assert_eq!(snap.meta.len(), 1);
        assert!(snap.frames.is_empty());
    }

    fn params(fps: Option<f32>, segments: Option<&str>) -> FeedParams {
        FeedParams {
            fps,
            segments: segments.map(str::to_string),
        }
    }

    #[test]
    fn frame_header_is_little_endian_and_precedes_the_payload() {
        let rgba = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let frame = encode_frame(0x0A0B0C0D, 2, 1, 0x11223344, &rgba);

        assert_eq!(frame.len(), FRAME_HEADER_BYTES + rgba.len());
        assert_eq!(&frame[0..4], &[0x0D, 0x0C, 0x0B, 0x0A]);
        assert_eq!(&frame[4..6], &[2, 0]);
        assert_eq!(&frame[6..8], &[1, 0]);
        assert_eq!(&frame[8..12], &[0x44, 0x33, 0x22, 0x11]);
        assert_eq!(&frame[FRAME_HEADER_BYTES..], &rgba[..]);
    }

    #[test]
    fn fps_is_clamped_to_a_sane_range() {
        assert_eq!(
            params(None, None).interval(),
            Duration::from_secs_f32(1.0 / 30.0)
        );
        assert_eq!(params(Some(0.0), None).interval(), Duration::from_secs(1));
        assert_eq!(
            params(Some(1000.0), None).interval(),
            Duration::from_secs_f32(1.0 / 60.0)
        );
    }

    #[test]
    fn segment_filter_ignores_unparseable_ids_but_keeps_the_rest() {
        assert_eq!(params(None, None).filter(), None);
        assert_eq!(params(None, Some("")).filter(), None);
        assert_eq!(params(None, Some("nonsense")).filter(), None);
        assert_eq!(params(None, Some("3")).filter(), Some(vec![3]));
        assert_eq!(params(None, Some("1, 2,x,3")).filter(), Some(vec![1, 2, 3]));
    }
}
