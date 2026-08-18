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
//! specific interface rather than `0.0.0.0`. The one principal that can cross
//! the network boundary anyway is a browser — any web page may open a
//! WebSocket, CORS notwithstanding — so [`ORIGINS_VAR`] can pin the `Origin`s
//! allowed to connect.

use axum::Router;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
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
/// cannot buffer without limit. Applied as both the frame and the message
/// limit: the frame cap alone would still let tungstenite reassemble
/// fragmented messages up to its 64 MiB default.
const MAX_CLIENT_FRAME: usize = 1024;

/// Idle interval after which a Ping goes out. Not for the client's benefit:
/// writes are what let TCP notice a vanished peer, and a held look sends
/// nothing, so without this a dead connection would hold one of the
/// [`MAX_CONNECTIONS`] permits forever.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// Environment variable holding a comma-separated `Origin` allowlist
/// (e.g. `https://vis.example,null` — `null` being what a `file://` page
/// sends). Unset means any origin may connect.
const ORIGINS_VAR: &str = "CUEPOOL_PIXELS_ORIGINS";

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

    let origins: Vec<String> = std::env::var(ORIGINS_VAR)
        .ok()
        .map(|list| {
            list.split(',')
                .map(|origin| origin.trim().trim_end_matches('/').to_string())
                .filter(|origin| !origin.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let state = FeedState {
        shared,
        connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        origins: Arc::new(origins),
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
    /// Exact `Origin` values allowed to connect; empty = no restriction.
    origins: Arc<Vec<String>>,
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
        // Non-finite is filtered before the clamp: clamp propagates NaN, and
        // `Duration::from_secs_f32(NaN)` panics — which the release profile's
        // `panic = "abort"` turns into one request killing the process.
        let fps = self
            .fps
            .filter(|fps| fps.is_finite())
            .unwrap_or(DEFAULT_FPS)
            .clamp(MIN_FPS, MAX_FPS);
        Duration::from_secs_f32(1.0 / fps)
    }

    /// `None` = no filter. Unparseable ids are dropped rather than failing the
    /// upgrade: a visualiser with one stale id in its list should still render
    /// the rest. A list with no parseable id at all fails closed — the client
    /// asked for a subset it could not name, which must not widen to
    /// everything.
    fn filter(&self) -> Option<Vec<u32>> {
        let raw = self.segments.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(
            raw.split(',')
                .filter_map(|id| id.trim().parse().ok())
                .collect(),
        )
    }
}

async fn upgrade(
    State(state): State<FeedState>,
    Query(params): Query<FeedParams>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Browsers attach `Origin` to every WebSocket handshake and CORS does not
    // apply, so without this gate a drive-by page could read the feed and pin
    // its permits. Requests without the header — non-browser clients — pass:
    // they are covered by the network reasoning in the module doc.
    if !state.origins.is_empty()
        && let Some(origin) = headers.get(header::ORIGIN)
    {
        let origin = origin.to_str().unwrap_or_default().trim_end_matches('/');
        if !state
            .origins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(origin))
        {
            log::warn!("CuePool pixel feed refused origin '{origin}'");
            return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }
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
        .max_message_size(MAX_CLIENT_FRAME)
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
    let mut last_write = Instant::now();
    let mut last_meta: Option<Vec<SegmentMeta>> = None;
    let mut last_pixels: HashMap<u32, Arc<Vec<u8>>> = HashMap::new();

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

        // A held look sends nothing, so idle connections ping: the write is
        // what lets TCP notice a vanished peer and hand its permit back.
        if last_write.elapsed() >= KEEPALIVE {
            if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                return;
            }
            last_write = Instant::now();
        }

        let elapsed_ms = started.elapsed().as_millis() as u32;
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
            last_write = Instant::now();
            // Only a resized grid invalidates its cached payload; name, gamma
            // or patch edits keep it — their pixels are unchanged, and wiping
            // the cache would resend every grid in full.
            if let Some(last) = &last_meta {
                evict_resized(last, &snapshot.meta, &mut last_pixels);
            }
            last_meta = Some(snapshot.meta);
        }

        for (id, frame, rgba) in snapshot.frames {
            if socket.send(Message::Binary(frame.into())).await.is_err() {
                return;
            }
            last_write = Instant::now();
            last_pixels.insert(id, rgba);
        }
    }
}

/// Drop cached payloads for segments whose grid changed between two metadata
/// frames: a same-length byte compare could otherwise skip the frame that
/// carries the new header dimensions.
fn evict_resized(
    last: &[SegmentMeta],
    next: &[SegmentMeta],
    cache: &mut HashMap<u32, Arc<Vec<u8>>>,
) {
    for meta in next {
        let resized = last
            .iter()
            .find(|previous| previous.id == meta.id)
            .is_none_or(|previous| (previous.cols, previous.rows) != (meta.cols, meta.rows));
        if resized {
            cache.remove(&meta.id);
        }
    }
}

struct Snapshot {
    meta: Vec<SegmentMeta>,
    /// `(segment id, encoded frame, sampler payload to cache once sent)`.
    frames: Vec<(u32, Vec<u8>, Arc<Vec<u8>>)>,
}

/// Copy out geometry and pixel handles under a single lock, then diff and
/// encode after it is released — the sampler and GUI need this mutex every
/// frame, so only cheap `Arc` clones happen inside it. `None` if the lock is
/// poisoned — a tick is skipped rather than killing the connection.
fn snapshot(
    shared: &SharedStateHandle,
    filter: Option<&[u32]>,
    last_pixels: &HashMap<u32, Arc<Vec<u8>>>,
    elapsed_ms: u32,
) -> Option<Snapshot> {
    let (meta, sampled) = {
        let state = shared.lock().ok()?;
        // The sampler is gated on the global toggle, so with lighting off the
        // preview map is only history: advertise nothing rather than serving
        // stale samples as live.
        if !state.show_file.lighting.enabled {
            return Some(Snapshot {
                meta: Vec::new(),
                frames: Vec::new(),
            });
        }

        let meta: Vec<SegmentMeta> = state
            .show_file
            .lighting
            .active_segments()
            .filter(|segment| filter.is_none_or(|ids| ids.contains(&segment.id)))
            .map(SegmentMeta::from)
            .collect();

        let sampled: Vec<(u32, u32, u32, Arc<Vec<u8>>)> = meta
            .iter()
            .filter_map(|segment| {
                let (cols, rows, rgba) = state.lighting_preview.get(&segment.id)?;
                Some((segment.id, *cols, *rows, Arc::clone(rgba)))
            })
            .collect();
        (meta, sampled)
    };

    let frames = sampled
        .into_iter()
        .filter_map(|(id, cols, rows, rgba)| {
            // Guard the cast: `cols`/`rows` are clamped to 512 by the sampler,
            // but the preview map is public state.
            let (cols, rows) = (u16::try_from(cols).ok()?, u16::try_from(rows).ok()?);
            if rgba.len() != cols as usize * rows as usize * 4 {
                return None; // mid-resize; the next tick will be consistent
            }
            if last_pixels
                .get(&id)
                .is_some_and(|last| Arc::ptr_eq(last, &rgba) || last == &rgba)
            {
                return None; // unchanged since last send — static content is free
            }
            Some((id, encode_frame(id, cols, rows, elapsed_ms, &rgba), rgba))
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
///
/// The timestamp wraps about every 49.7 days on one connection; clients
/// comparing times should subtract with wrapping arithmetic.
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
#[derive(Debug, Clone, Serialize)]
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

/// Hand-written so the floats compare bitwise: change detection must see NaN
/// as equal to itself, or one NaN gamma/region would resend metadata — and
/// through [`evict_resized`], pixels — every tick. Keep in step with the
/// fields above.
impl PartialEq for SegmentMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.cols == other.cols
            && self.rows == other.rows
            && self.region.map(f32::to_bits) == other.region.map(f32::to_bits)
            && self.source == other.source
            && self.universe == other.universe
            && self.address == other.address
            && self.gamma.to_bits() == other.gamma.to_bits()
            && self.order == other.order
    }
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            shared
                .lighting_preview
                .insert(id, (cols, rows, Arc::new(rgba)));
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
        assert_eq!(**rgba, vec![9u8; 8]);
        assert_eq!(frame.len(), FRAME_HEADER_BYTES + 8);
        assert_eq!(&frame[0..4], &7u32.to_le_bytes());
        assert_eq!(&frame[4..6], &2u16.to_le_bytes());
        assert_eq!(&frame[6..8], &1u16.to_le_bytes());
    }

    #[test]
    fn unchanged_pixels_send_nothing_but_still_report_geometry() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![9u8; 8])]);
        let cached = HashMap::from([(7, Arc::new(vec![9u8; 8]))]);

        let snap = snapshot(&shared, None, &cached, 5).expect("lock held");

        assert_eq!(snap.meta.len(), 1, "geometry is still advertised");
        assert!(snap.frames.is_empty(), "static content should cost nothing");
    }

    #[test]
    fn lighting_disabled_globally_yields_no_metadata_and_no_frames() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![9u8; 8])]);
        shared.lock().unwrap().show_file.lighting.enabled = false;
        let empty = HashMap::new();

        let snap = snapshot(&shared, None, &empty, 0).expect("lock held");

        assert!(
            snap.meta.is_empty(),
            "the sampler is off; the preview map is only history"
        );
        assert!(snap.frames.is_empty());
    }

    #[test]
    fn changed_pixels_resend() {
        let shared = state(vec![segment(7, 2, 1)], vec![(7, 2, 1, vec![1u8; 8])]);
        let cached = HashMap::from([(7, Arc::new(vec![9u8; 8]))]);

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
    fn non_finite_fps_falls_back_to_the_default_instead_of_panicking() {
        let default = Duration::from_secs_f32(1.0 / 30.0);
        assert_eq!(params(Some(f32::NAN), None).interval(), default);
        assert_eq!(params(Some(f32::INFINITY), None).interval(), default);
        assert_eq!(params(Some(f32::NEG_INFINITY), None).interval(), default);
    }

    #[test]
    fn segment_filter_ignores_unparseable_ids_but_keeps_the_rest() {
        assert_eq!(params(None, None).filter(), None);
        assert_eq!(params(None, Some("")).filter(), None);
        assert_eq!(params(None, Some("3")).filter(), Some(vec![3]));
        assert_eq!(params(None, Some("1, 2,x,3")).filter(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn a_filter_with_no_parseable_id_fails_closed() {
        assert_eq!(params(None, Some("nonsense")).filter(), Some(vec![]));
        assert_eq!(params(None, Some("1;2")).filter(), Some(vec![]));
    }

    /// A [`SegmentMeta`] as `evict_resized` and the change detector see it.
    fn meta(id: u32, cols: u32, rows: u32, gamma: f32) -> SegmentMeta {
        SegmentMeta::from(&PixelMapSegment {
            cols,
            rows,
            gamma,
            ..PixelMapSegment::new(id)
        })
    }

    #[test]
    fn non_geometry_edits_keep_the_pixel_cache() {
        let last = vec![meta(7, 2, 1, 2.2)];
        let next = vec![meta(7, 2, 1, 1.8)];
        let mut cache = HashMap::from([(7, Arc::new(vec![9u8; 8]))]);

        evict_resized(&last, &next, &mut cache);

        assert!(
            cache.contains_key(&7),
            "a gamma edit must not force a full resend"
        );
    }

    #[test]
    fn a_resize_or_a_reused_id_evicts_the_cached_payload() {
        let last = vec![meta(7, 2, 1, 2.2)];
        let next = vec![meta(7, 4, 1, 2.2), meta(9, 2, 1, 2.2)];
        let mut cache = HashMap::from([(7, Arc::new(vec![9u8; 8])), (9, Arc::new(vec![9u8; 8]))]);

        evict_resized(&last, &next, &mut cache);

        assert!(!cache.contains_key(&7), "a resized grid must resend");
        assert!(
            !cache.contains_key(&9),
            "an id absent from the last metadata must resend"
        );
    }

    #[test]
    fn nan_gamma_does_not_defeat_meta_change_detection() {
        assert_eq!(
            meta(7, 2, 1, f32::NAN),
            meta(7, 2, 1, f32::NAN),
            "NaN must equal itself here, or metadata resends every tick"
        );
        assert_ne!(meta(7, 2, 1, f32::NAN), meta(7, 2, 1, 2.2));
    }

    fn feed_router(shared: SharedStateHandle, permits: usize, origins: Vec<String>) -> Router {
        Router::new()
            .route("/v1/pixels", get(upgrade))
            .with_state(FeedState {
                shared,
                connections: Arc::new(Semaphore::new(permits)),
                origins: Arc::new(origins),
            })
    }

    /// The real listener, on an ephemeral loopback port: axum's WebSocket
    /// extractor needs hyper's genuine upgrade plumbing, which a
    /// `tower::oneshot` request does not carry.
    async fn serve_feed(permits: usize, origins: Vec<String>) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = feed_router(state(vec![], vec![]), permits, origins);
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        address
    }

    /// Open a raw handshake and return the HTTP status line of the reply.
    async fn handshake(address: SocketAddr, origin: Option<&str>) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let origin_line = origin
            .map(|origin| format!("origin: {origin}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "GET /v1/pixels HTTP/1.1\r\n\
             host: {address}\r\n\
             connection: upgrade\r\n\
             upgrade: websocket\r\n\
             sec-websocket-version: 13\r\n\
             sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             {origin_line}\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = [0u8; 128];
        let read = stream.read(&mut reply).await.unwrap();
        String::from_utf8_lossy(&reply[..read])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn a_full_house_gets_503_and_a_free_permit_gets_the_upgrade() {
        let full = serve_feed(0, vec![]).await;
        assert!(handshake(full, None).await.contains("503"));

        let open = serve_feed(1, vec![]).await;
        assert!(handshake(open, None).await.contains("101"));
    }

    #[tokio::test]
    async fn the_origin_allowlist_refuses_unlisted_browsers_but_passes_native_clients() {
        let origins = vec!["https://vis.example".to_string()];
        let address = serve_feed(MAX_CONNECTIONS, origins).await;

        let refused = handshake(address, Some("https://evil.example")).await;
        assert!(refused.contains("403"), "unlisted origin: {refused}");

        let allowed = handshake(address, Some("https://vis.example")).await;
        assert!(allowed.contains("101"), "listed origin: {allowed}");

        let native = handshake(address, None).await;
        assert!(native.contains("101"), "no origin header: {native}");
    }
}
