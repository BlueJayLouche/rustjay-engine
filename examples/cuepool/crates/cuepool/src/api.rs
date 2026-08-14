use axum::extract::{Path, Query, State};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use cuepool_gui::app::CueState;
use cuepool_gui::{Diagnostics, SharedStateHandle, ShowMode};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, broadcast, mpsc};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{IntoParams, Modify, OpenApi, ToSchema};

const DEFAULT_BIND: &str = "127.0.0.1:7133";
const COMMAND_QUEUE_CAPACITY: usize = 64;
const COMMAND_HISTORY_CAPACITY: usize = 256;
const STATUS_HISTORY_CAPACITY: usize = 3600;
const MAIN_LOOP_STALE_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ApiCommand {
    OpenProject { path: String },
    SelectCue { qid: String },
    Go,
    Stop,
    Pause,
    Resume,
    Preload,
    Seek { instance_id: u64, seconds: f32 },
}

#[derive(Debug)]
pub struct ApiCommandRequest {
    pub id: u64,
    pub command: ApiCommand,
    pub prepared_project: Option<cuepool_gui::PreparedProject>,
}

#[derive(Debug)]
pub enum ApiCommandOutcome {
    Applied(String),
    Rejected(String),
}

#[derive(Debug)]
struct ApiCommandResult {
    id: u64,
    outcome: ApiCommandOutcome,
}

pub struct ApiRuntime {
    command_rx: mpsc::Receiver<ApiCommandRequest>,
    result_tx: mpsc::UnboundedSender<ApiCommandResult>,
    ready: Arc<AtomicBool>,
    main_loop_heartbeat: Arc<Mutex<Instant>>,
    _thread: std::thread::JoinHandle<()>,
}

impl ApiRuntime {
    pub fn try_recv(&mut self) -> Option<ApiCommandRequest> {
        self.command_rx.try_recv().ok()
    }

    pub fn complete(&self, id: u64, outcome: ApiCommandOutcome) {
        let _ = self.result_tx.send(ApiCommandResult { id, outcome });
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
        self.mark_alive();
    }

    pub fn mark_alive(&self) {
        if let Ok(mut heartbeat) = self.main_loop_heartbeat.lock() {
            *heartbeat = Instant::now();
        }
    }

    pub fn mark_stopping(&self) {
        self.ready.store(false, Ordering::Release);
    }
}

pub fn start(shared: SharedStateHandle) -> anyhow::Result<ApiRuntime> {
    let bind = std::env::var("CUEPOOL_API_BIND").unwrap_or_else(|_| DEFAULT_BIND.into());
    let address: SocketAddr = bind
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid CUEPOOL_API_BIND '{bind}': {error}"))?;
    validate_bind_address(address)?;
    let listener = TcpListener::bind(address)
        .map_err(|error| anyhow::anyhow!("cannot bind CuePool API to {address}: {error}"))?;
    listener.set_nonblocking(true)?;
    let local_addr = listener.local_addr()?;

    let control_token = std::env::var("CUEPOOL_API_CONTROL_TOKEN")
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());
    let control_enabled = control_token.is_some();

    let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let (result_tx, result_rx) = mpsc::unbounded_channel();
    let ready = Arc::new(AtomicBool::new(false));
    let main_loop_heartbeat = Arc::new(Mutex::new(Instant::now()));
    let state = ApiState::new(
        shared,
        control_token,
        command_tx,
        Arc::clone(&ready),
        Arc::clone(&main_loop_heartbeat),
    );
    let thread = std::thread::Builder::new()
        .name("cuepool-api".into())
        .spawn(move || run_server(listener, state, result_rx))?;

    log::info!(
        "CuePool API listening on http://{local_addr}/v1 (control {})",
        if control_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    Ok(ApiRuntime {
        command_rx,
        result_tx,
        ready,
        main_loop_heartbeat,
        _thread: thread,
    })
}

fn validate_bind_address(address: SocketAddr) -> anyhow::Result<()> {
    anyhow::ensure!(
        address.ip().is_loopback(),
        "CUEPOOL_API_BIND must use a loopback address; expose the API through an authenticated TLS tunnel or reverse proxy"
    );
    Ok(())
}

fn run_server(
    listener: TcpListener,
    state: ApiState,
    result_rx: mpsc::UnboundedReceiver<ApiCommandResult>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = runtime.map_err(anyhow::Error::from).and_then(|runtime| {
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            tokio::spawn(record_command_results(state.clone(), result_rx));
            tokio::spawn(sample_state(state.clone()));
            axum::serve(listener, build_router(state)).await?;
            Ok(())
        })
    });
    if let Err(error) = result {
        log::error!("CuePool API stopped: {error}");
    }
}

#[derive(Clone)]
struct ApiState {
    shared: SharedStateHandle,
    started_at: Instant,
    control_token: Option<Arc<str>>,
    command_tx: mpsc::Sender<ApiCommandRequest>,
    ready: Arc<AtomicBool>,
    main_loop_heartbeat: Arc<Mutex<Instant>>,
    next_command_id: Arc<AtomicU64>,
    commands: Arc<Mutex<VecDeque<CommandRecord>>>,
    retired_idempotency_keys: Arc<Mutex<HashSet<String>>>,
    status_history: Arc<Mutex<VecDeque<StatusSample>>>,
    events: broadcast::Sender<ApiEvent>,
    project_preparation: Arc<Semaphore>,
}

impl ApiState {
    fn new(
        shared: SharedStateHandle,
        control_token: Option<String>,
        command_tx: mpsc::Sender<ApiCommandRequest>,
        ready: Arc<AtomicBool>,
        main_loop_heartbeat: Arc<Mutex<Instant>>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            shared,
            started_at: Instant::now(),
            control_token: control_token.map(Arc::from),
            command_tx,
            ready,
            main_loop_heartbeat,
            next_command_id: Arc::new(AtomicU64::new(1)),
            commands: Arc::new(Mutex::new(VecDeque::new())),
            retired_idempotency_keys: Arc::new(Mutex::new(HashSet::new())),
            status_history: Arc::new(Mutex::new(VecDeque::new())),
            events,
            project_preparation: Arc::new(Semaphore::new(2)),
        }
    }

    fn register_command(
        &self,
        command: ApiCommand,
        prepared_project: Option<cuepool_gui::PreparedProject>,
        idempotency_key: Option<String>,
    ) -> Result<CommandStatus, ApiError> {
        let mut commands = self
            .commands
            .lock()
            .map_err(|_| ApiError::internal("command history lock poisoned"))?;
        if let Some(key) = idempotency_key.as_deref()
            && let Some(existing) = commands
                .iter()
                .find(|record| record.idempotency_key.as_deref() == Some(key))
        {
            if existing.command != command {
                return Err(ApiError::conflict(
                    "idempotency key was already used for a different command",
                ));
            }
            return Ok(existing.status.clone());
        }
        if idempotency_key.as_ref().is_some_and(|key| {
            self.retired_idempotency_keys
                .lock()
                .is_ok_and(|keys| keys.contains(key))
        }) {
            return Err(ApiError::conflict(
                "idempotency key has expired; use a new key only for a new command",
            ));
        }
        let permit = self.command_tx.try_reserve().map_err(|error| {
            ApiError::unavailable(format!("command queue unavailable: {error}"))
        })?;
        let id = self.next_command_id.fetch_add(1, Ordering::Relaxed);
        let status = CommandStatus {
            id,
            state: CommandState::Pending,
            message: None,
            created_at: now(),
            completed_at: None,
        };
        while commands.len() >= COMMAND_HISTORY_CAPACITY {
            let Some(index) = commands
                .iter()
                .position(|record| record.status.state != CommandState::Pending)
            else {
                return Err(ApiError::unavailable("command result history is full"));
            };
            if let Some(key) = commands
                .remove(index)
                .and_then(|record| record.idempotency_key)
                && let Ok(mut keys) = self.retired_idempotency_keys.lock()
            {
                keys.insert(key);
            }
        }
        commands.push_back(CommandRecord {
            status: status.clone(),
            idempotency_key,
            command: command.clone(),
        });
        drop(commands);
        permit.send(ApiCommandRequest {
            id,
            command,
            prepared_project,
        });
        Ok(status)
    }

    fn idempotent_command(
        &self,
        key: &str,
        command: &ApiCommand,
    ) -> Result<Option<CommandStatus>, ApiError> {
        let commands = self
            .commands
            .lock()
            .map_err(|_| ApiError::internal("command history lock poisoned"))?;
        let Some(existing) = commands
            .iter()
            .find(|record| record.idempotency_key.as_deref() == Some(key))
        else {
            if self
                .retired_idempotency_keys
                .lock()
                .is_ok_and(|keys| keys.contains(key))
            {
                return Err(ApiError::conflict(
                    "idempotency key has expired; use a new key only for a new command",
                ));
            }
            return Ok(None);
        };
        if existing.command != *command {
            return Err(ApiError::conflict(
                "idempotency key was already used for a different command",
            ));
        }
        Ok(Some(existing.status.clone()))
    }

    fn complete_command(&self, result: ApiCommandResult) {
        let completed_at = now();
        let mut completed = None;
        if let Ok(mut commands) = self.commands.lock()
            && let Some(command) = commands
                .iter_mut()
                .find(|record| record.status.id == result.id)
        {
            match result.outcome {
                ApiCommandOutcome::Applied(message) => {
                    command.status.state = CommandState::Applied;
                    command.status.message = Some(message);
                }
                ApiCommandOutcome::Rejected(message) => {
                    command.status.state = CommandState::Rejected;
                    command.status.message = Some(message);
                }
            }
            command.status.completed_at = Some(completed_at);
            completed = Some(command.status.clone());
        }
        if let Some(command) = completed {
            self.emit("command", &command);
        }
    }

    fn emit<T: Serialize>(&self, event_type: &'static str, value: &T) {
        if let Ok(data) = serde_json::to_value(value) {
            let _ = self.events.send(ApiEvent { event_type, data });
        }
    }

    fn main_loop_responsive(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && self
                .main_loop_heartbeat
                .lock()
                .is_ok_and(|heartbeat| heartbeat.elapsed() < MAIN_LOOP_STALE_AFTER)
    }
}

#[derive(Clone)]
struct ApiEvent {
    event_type: &'static str,
    data: serde_json::Value,
}

async fn record_command_results(
    state: ApiState,
    mut results: mpsc::UnboundedReceiver<ApiCommandResult>,
) {
    while let Some(result) = results.recv().await {
        state.complete_command(result);
    }
}

async fn sample_state(state: ApiState) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_log_cursor = 0;
    loop {
        interval.tick().await;
        if state.main_loop_responsive()
            && let Ok(status) = diagnostics_snapshot(&state.shared)
        {
            let sample = StatusSample {
                captured_at: now(),
                status,
            };
            if let Ok(mut history) = state.status_history.lock() {
                history.push_back(sample.clone());
                while history.len() > STATUS_HISTORY_CAPACITY {
                    history.pop_front();
                }
            }
            state.emit("status", &sample);
        }

        let logs = logs_after(last_log_cursor);
        if let Some(cursor) = logs.entries.last().map(|entry| entry.cursor) {
            last_log_cursor = cursor;
            state.emit("logs", &logs);
        }
    }
}

fn build_router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/openapi.json", get(openapi))
        .route("/v1/health", get(health))
        .route("/v1/project", get(project))
        .route("/v1/cues", get(cues))
        .route("/v1/cues/active", get(active_cues))
        .route("/v1/status", get(status))
        .route("/v1/status/history", get(status_history))
        .route("/v1/logs", get(logs))
        .route("/v1/events", get(events))
        .route("/v1/commands", post(post_command))
        .route("/v1/commands/{id}", get(command_status))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    info(
        title = "CuePool Automation API",
        version = "1.0.0",
        description = "Versioned diagnostics and acknowledged show-control API. Reads are available whenever CuePool is running. Commands require a bearer token configured with CUEPOOL_API_CONTROL_TOKEN."
    ),
    paths(
        health,
        project,
        cues,
        active_cues,
        status,
        status_history,
        logs,
        events,
        post_command,
        command_status,
    ),
    components(schemas(
        HealthResponse,
        ProjectResponse,
        CueResponse,
        ActiveCueResponse,
        DiagnosticsResponse,
        GpuStatus,
        OutputStatus,
        VideoStatus,
        VideoTimingStatus,
        PacingStatus,
        EnvironmentOverride,
        StatusSample,
        StatusHistoryResponse,
        LogEntryResponse,
        LogsResponse,
        ApiCommand,
        CommandStatus,
        CommandState,
        ErrorResponse,
    )),
    tags(
        (name = "Read", description = "CuePool state and diagnostics"),
        (name = "Control", description = "Authenticated, acknowledged commands"),
        (name = "Stream", description = "Server-sent status, log, and command events")
    )
)]
struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_token",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorResponse {
    error: String,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    error: &'static str,
    message: String,
}

impl ApiError {
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "internal",
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "bad_request",
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            error: "conflict",
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            error: "not_found",
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error: "unavailable",
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let unauthorized = self.status == StatusCode::UNAUTHORIZED;
        let mut response = (
            self.status,
            Json(ErrorResponse {
                error: self.error.into(),
                message: self.message,
            }),
        )
            .into_response();
        if unauthorized {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

fn read_shared<T>(
    state: &SharedStateHandle,
    read: impl FnOnce(&cuepool_gui::SharedState) -> T,
) -> Result<T, ApiError> {
    state
        .lock()
        .map(|state| read(&state))
        .map_err(|_| ApiError::internal("CuePool state lock poisoned"))
}

#[derive(Debug, Serialize, ToSchema)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    commit: Option<&'static str>,
    pid: u32,
    uptime_seconds: u64,
    ready: bool,
    project_path: Option<String>,
    dirty: bool,
    selected_cue: Option<String>,
    active_cues: usize,
    active_decode_path: Option<String>,
    control_enabled: bool,
}

#[utoipa::path(
    get,
    path = "/v1/health",
    responses(
        (status = 200, body = HealthResponse),
        (status = 500, body = ErrorResponse)
    ),
    tag = "Read"
)]
async fn health(State(api): State<ApiState>) -> Result<Json<HealthResponse>, ApiError> {
    let response = read_shared(&api.shared, |state| {
        let started = api.ready.load(Ordering::Acquire);
        let responsive = api.main_loop_responsive();
        let ready = responsive
            && state.audio_error.is_none()
            && state.diagnostics.consumer_error.is_none();
        HealthResponse {
            status: if ready {
                "ok"
            } else if started {
                "degraded"
            } else {
                "starting"
            },
            version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("CUEPOOL_BUILD_ID"),
            pid: std::process::id(),
            uptime_seconds: api.started_at.elapsed().as_secs(),
            ready,
            project_path: state
                .project_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            dirty: state.dirty,
            selected_cue: state.selected_cue_id.map(|qid| qid.to_string()),
            active_cues: state.active_cues.len(),
            active_decode_path: state
                .diagnostics
                .video
                .as_ref()
                .map(|video| video.decode_path.clone()),
            control_enabled: api.control_token.is_some(),
        }
    })?;
    Ok(Json(response))
}

#[derive(Debug, Serialize, ToSchema)]
struct ProjectResponse {
    path: Option<String>,
    dirty: bool,
    show_mode: String,
    selected_cue: Option<String>,
    cue_count: usize,
    active_cue_count: usize,
    show_time_seconds: Option<f64>,
    paused: bool,
}

#[utoipa::path(
    get,
    path = "/v1/project",
    responses(
        (status = 200, body = ProjectResponse),
        (status = 500, body = ErrorResponse)
    ),
    tag = "Read"
)]
async fn project(State(api): State<ApiState>) -> Result<Json<ProjectResponse>, ApiError> {
    read_shared(&api.shared, |state| ProjectResponse {
        path: state
            .project_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        dirty: state.dirty,
        show_mode: match state.show_mode {
            ShowMode::Edit => "edit",
            ShowMode::Show => "show",
        }
        .into(),
        selected_cue: state.selected_cue_id.map(|qid| qid.to_string()),
        cue_count: state.show_file.cues.len(),
        active_cue_count: state.active_cues.len(),
        show_time_seconds: state.show_time,
        paused: state.show_paused,
    })
    .map(Json)
}

#[derive(Debug, Serialize, ToSchema)]
struct CueResponse {
    qid: String,
    name: String,
    cue_type: &'static str,
    enabled: bool,
    selected: bool,
    parent: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/cues",
    responses(
        (status = 200, body = [CueResponse]),
        (status = 500, body = ErrorResponse)
    ),
    tag = "Read"
)]
async fn cues(State(api): State<ApiState>) -> Result<Json<Vec<CueResponse>>, ApiError> {
    read_shared(&api.shared, |state| {
        state
            .show_file
            .cues
            .iter()
            .map(|cue| {
                let base = cue.base();
                CueResponse {
                    qid: base.qid.to_string(),
                    name: base.name.clone(),
                    cue_type: cue_type(cue),
                    enabled: cue.enabled(),
                    selected: state.selected_cue_id == Some(base.qid),
                    parent: base.parent.map(|qid| qid.to_string()),
                }
            })
            .collect()
    })
    .map(Json)
}

fn cue_type(cue: &cuepool_core::Cue) -> &'static str {
    match cue {
        cuepool_core::Cue::Sound { .. } => "sound",
        cuepool_core::Cue::Video { .. } => "video",
        cuepool_core::Cue::Stop { .. } => "stop",
        cuepool_core::Cue::Volume { .. } => "volume",
        cuepool_core::Cue::Group { .. } => "group",
        cuepool_core::Cue::Dummy { .. } => "dummy",
        cuepool_core::Cue::TimeCode { .. } => "time_code",
        cuepool_core::Cue::Osc { .. } => "osc",
        cuepool_core::Cue::Text { .. } => "text",
        cuepool_core::Cue::Image { .. } => "image",
        cuepool_core::Cue::Goto { .. } => "goto",
        cuepool_core::Cue::Lighting { .. } => "lighting",
        cuepool_core::Cue::DmxShow { .. } => "dmx_show",
        cuepool_core::Cue::PixelMap { .. } => "pixel_map",
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct ActiveCueResponse {
    instance_id: u64,
    qid: String,
    name: String,
    paused: bool,
    position_seconds: f32,
    length_seconds: Option<f32>,
    state: &'static str,
}

#[utoipa::path(
    get,
    path = "/v1/cues/active",
    responses(
        (status = 200, body = [ActiveCueResponse]),
        (status = 500, body = ErrorResponse)
    ),
    tag = "Read"
)]
async fn active_cues(
    State(api): State<ApiState>,
) -> Result<Json<Vec<ActiveCueResponse>>, ApiError> {
    read_shared(&api.shared, |state| {
        state
            .active_cues
            .iter()
            .map(|cue| ActiveCueResponse {
                instance_id: cue.instance_id,
                qid: cue.qid.to_string(),
                name: cue.name.clone(),
                paused: cue.paused,
                position_seconds: cue.position_secs,
                length_seconds: cue.length_secs,
                state: cue_state(cue.state),
            })
            .collect()
    })
    .map(Json)
}

fn cue_state(state: CueState) -> &'static str {
    match state {
        CueState::Ready => "ready",
        CueState::Delay => "delay",
        CueState::Playing => "playing",
        CueState::PlayingLooped => "playing_looped",
        CueState::Paused => "paused",
        CueState::Done => "done",
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct DiagnosticsResponse {
    app_version: String,
    os: String,
    arch: String,
    gpu: GpuStatus,
    ffmpeg_version: String,
    environment_overrides: Vec<EnvironmentOverride>,
    outputs: Vec<OutputStatus>,
    video: Option<VideoStatus>,
    pacing: PacingStatus,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct GpuStatus {
    name: String,
    backend: String,
    driver: String,
    driver_info: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct EnvironmentOverride {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct OutputStatus {
    name: String,
    width: u32,
    height: u32,
    present_mode: String,
    format: String,
    refresh: String,
    fullscreen: bool,
    presented_per_second: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct VideoStatus {
    path: String,
    width: u32,
    height: u32,
    decode_path: String,
    fallback_reason: Option<String>,
    timings: VideoTimingStatus,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct VideoTimingStatus {
    decode_ms_per_frame: f64,
    hardware_transfer_ms_per_frame: f64,
    plane_copy_ms_per_frame: f64,
    upload_ms_per_frame: f64,
    conversion_submit_ms_per_frame: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct PacingStatus {
    presented_per_second: f64,
    starved_per_second: f64,
    uploads_per_second: f64,
    dropped_per_second: f64,
    event_loop_per_second: f64,
    consumer_error: Option<String>,
}

fn diagnostics_from(diagnostics: &Diagnostics) -> DiagnosticsResponse {
    DiagnosticsResponse {
        app_version: diagnostics.app_version.clone(),
        os: diagnostics.os.clone(),
        arch: diagnostics.arch.clone(),
        gpu: GpuStatus {
            name: diagnostics.gpu_name.clone(),
            backend: diagnostics.gpu_backend.clone(),
            driver: diagnostics.gpu_driver.clone(),
            driver_info: diagnostics.gpu_driver_info.clone(),
        },
        ffmpeg_version: diagnostics.ffmpeg_version.clone(),
        environment_overrides: diagnostics
            .env_overrides
            .iter()
            .map(|(name, value)| EnvironmentOverride {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        outputs: diagnostics
            .outputs
            .iter()
            .map(|output| OutputStatus {
                name: output.name.clone(),
                width: output.size.0,
                height: output.size.1,
                present_mode: output.present_mode.clone(),
                format: output.format.clone(),
                refresh: output.refresh.clone(),
                fullscreen: output.fullscreen,
                presented_per_second: output.presented_per_sec,
            })
            .collect(),
        video: diagnostics.video.as_ref().map(|video| VideoStatus {
            path: video.path.clone(),
            width: video.width,
            height: video.height,
            decode_path: video.decode_path.clone(),
            fallback_reason: video.fallback_reason.clone(),
            timings: VideoTimingStatus {
                decode_ms_per_frame: video.timings.decode.get_ms(),
                hardware_transfer_ms_per_frame: video.timings.hw_transfer.get_ms(),
                plane_copy_ms_per_frame: video.timings.plane_copy.get_ms(),
                upload_ms_per_frame: video.timings.upload.get_ms(),
                conversion_submit_ms_per_frame: video.timings.conversion_submit.get_ms(),
            },
        }),
        pacing: PacingStatus {
            presented_per_second: diagnostics.presented_per_sec,
            starved_per_second: diagnostics.starved_per_sec,
            uploads_per_second: diagnostics.uploads_per_sec,
            dropped_per_second: diagnostics.dropped_per_sec,
            event_loop_per_second: diagnostics.event_loop_per_sec,
            consumer_error: diagnostics.consumer_error.clone(),
        },
    }
}

fn diagnostics_snapshot(state: &SharedStateHandle) -> Result<DiagnosticsResponse, ApiError> {
    read_shared(state, |state| diagnostics_from(&state.diagnostics))
}

#[utoipa::path(
    get,
    path = "/v1/status",
    responses(
        (status = 200, body = DiagnosticsResponse),
        (status = 500, body = ErrorResponse)
    ),
    tag = "Read"
)]
async fn status(State(api): State<ApiState>) -> Result<Json<DiagnosticsResponse>, ApiError> {
    diagnostics_snapshot(&api.shared).map(Json)
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct StatusSample {
    captured_at: String,
    status: DiagnosticsResponse,
}

#[derive(Debug, Deserialize, IntoParams)]
struct HistoryQuery {
    /// Number of recent one-second samples to return, from 1 to 3600.
    seconds: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
struct StatusHistoryResponse {
    seconds: usize,
    samples: Vec<StatusSample>,
}

#[utoipa::path(
    get,
    path = "/v1/status/history",
    params(HistoryQuery),
    responses((status = 200, body = StatusHistoryResponse)),
    tag = "Read"
)]
async fn status_history(
    State(api): State<ApiState>,
    Query(query): Query<HistoryQuery>,
) -> Json<StatusHistoryResponse> {
    let seconds = query
        .seconds
        .unwrap_or(300)
        .clamp(1, STATUS_HISTORY_CAPACITY);
    let samples = api
        .status_history
        .lock()
        .map(|history| {
            history
                .iter()
                .skip(history.len().saturating_sub(seconds))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Json(StatusHistoryResponse { seconds, samples })
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct LogEntryResponse {
    cursor: u64,
    recorded_at: String,
    level: String,
    target: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct LogsResponse {
    entries: Vec<LogEntryResponse>,
    next_cursor: u64,
    truncated: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
struct LogsQuery {
    /// Return records with a cursor greater than this value.
    after: Option<u64>,
}

fn logs_after(after: u64) -> LogsResponse {
    let entries = cuepool_gui::logging::read_log_buffer();
    let truncated = after > 0
        && entries
            .first()
            .is_some_and(|entry| entry.cursor > after.saturating_add(1));
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.cursor > after)
        .map(|entry| LogEntryResponse {
            cursor: entry.cursor,
            recorded_at: entry.recorded_at,
            level: entry.level.to_string().to_lowercase(),
            target: entry.target,
            message: entry.message,
        })
        .collect();
    let next_cursor = entries.last().map_or(after, |entry| entry.cursor);
    LogsResponse {
        entries,
        next_cursor,
        truncated,
    }
}

#[utoipa::path(
    get,
    path = "/v1/logs",
    params(LogsQuery),
    responses((status = 200, body = LogsResponse)),
    tag = "Read"
)]
async fn logs(Query(query): Query<LogsQuery>) -> Json<LogsResponse> {
    Json(logs_after(query.after.unwrap_or(0)))
}

#[utoipa::path(
    get,
    path = "/v1/events",
    responses((status = 200, description = "SSE stream of status, logs, and command results")),
    tag = "Stream"
)]
async fn events(
    State(api): State<ApiState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(api.events.subscribe()).filter_map(|event| {
        let event = match event {
            Ok(event) => match Event::default()
                .event(event.event_type)
                .json_data(event.data)
            {
                Ok(event) => event,
                Err(_) => Event::default().event("error").data("serialization failed"),
            },
            Err(_) => Event::default().event("lagged").data("events dropped"),
        };
        Some(Ok(event))
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum CommandState {
    Pending,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct CommandStatus {
    id: u64,
    state: CommandState,
    message: Option<String>,
    created_at: String,
    completed_at: Option<String>,
}

struct CommandRecord {
    status: CommandStatus,
    idempotency_key: Option<String>,
    command: ApiCommand,
}

fn validate_command(command: &ApiCommand) -> Result<(), ApiError> {
    match command {
        ApiCommand::OpenProject { path } => {
            if path.trim().is_empty() {
                return Err(ApiError::bad_request("project path must not be empty"));
            }
            if !PathBuf::from(path).is_absolute() {
                return Err(ApiError::bad_request("project path must be absolute"));
            }
        }
        ApiCommand::SelectCue { qid } if qid.trim().is_empty() => {
            return Err(ApiError::bad_request("cue qid must not be empty"));
        }
        ApiCommand::Seek { seconds, .. } if !seconds.is_finite() || *seconds < 0.0 => {
            return Err(ApiError::bad_request(
                "seek seconds must be finite and non-negative",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn authorize_control(api: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(expected) = api.control_token.as_deref() else {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            error: "control_disabled",
            message: "set CUEPOOL_API_CONTROL_TOKEN to enable commands".into(),
        });
    };
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !supplied.is_some_and(|supplied| tokens_equal(expected, supplied)) {
        return Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            message: "valid bearer token required".into(),
        });
    }
    Ok(())
}

fn tokens_equal(expected: &str, supplied: &str) -> bool {
    expected.len() == supplied.len()
        && expected
            .bytes()
            .zip(supplied.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[utoipa::path(
    post,
    path = "/v1/commands",
    security(("bearer_token" = [])),
    request_body = ApiCommand,
    responses(
        (status = 202, body = CommandStatus),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    tag = "Control"
)]
async fn post_command(
    State(api): State<ApiState>,
    headers: HeaderMap,
    Json(command): Json<ApiCommand>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_control(&api, &headers)?;
    if !api.main_loop_responsive() {
        return Err(ApiError::unavailable(
            "CuePool show-control loop is not ready",
        ));
    }
    validate_command(&command)?;
    let idempotency_key = headers
        .get("idempotency-key")
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_string)
                .map_err(|_| ApiError::bad_request("Idempotency-Key must be valid ASCII"))
        })
        .transpose()?;
    if idempotency_key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.len() > 128)
    {
        return Err(ApiError::bad_request(
            "Idempotency-Key must contain 1 to 128 characters",
        ));
    }
    if let Some(key) = idempotency_key.as_deref()
        && let Some(status) = api.idempotent_command(key, &command)?
    {
        return Ok((StatusCode::ACCEPTED, Json(status)));
    }
    let prepared_project = match &command {
        ApiCommand::OpenProject { path } => {
            let permit = Arc::clone(&api.project_preparation)
                .try_acquire_owned()
                .map_err(|_| ApiError::unavailable("project preparation is busy"))?;
            let path = PathBuf::from(path);
            Some(
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    cuepool_gui::prepare_unattended_project(&path)
                })
                    .await
                    .map_err(|error| {
                        ApiError::internal(format!("project validation failed: {error}"))
                    })?
                    .map_err(ApiError::bad_request)?,
            )
        }
        _ => None,
    };
    let status = api.register_command(command, prepared_project, idempotency_key)?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

#[utoipa::path(
    get,
    path = "/v1/commands/{id}",
    params(("id" = u64, Path, description = "Command ID returned by POST /v1/commands")),
    responses(
        (status = 200, body = CommandStatus),
        (status = 404, body = ErrorResponse)
    ),
    tag = "Control"
)]
async fn command_status(
    State(api): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Json<CommandStatus>, ApiError> {
    let command = api
        .commands
        .lock()
        .ok()
        .and_then(|commands| {
            commands
                .iter()
                .find(|record| record.status.id == id)
                .map(|record| record.status.clone())
        })
        .ok_or_else(|| ApiError::not_found(format!("command {id} not found")))?;
    Ok(Json(command))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_api(token: Option<&str>) -> (ApiState, mpsc::Receiver<ApiCommandRequest>) {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let state = ApiState::new(
            Arc::new(Mutex::new(cuepool_gui::SharedState::default())),
            token.map(str::to_string),
            command_tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(Instant::now())),
        );
        (state, command_rx)
    }

    async fn json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn read_routes_and_openapi_are_available_without_control() {
        let (state, _) = test_api(None);
        let app = build_router(state);

        let response = app
            .clone()
            .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let health = json(response).await;
        assert_eq!(health["status"], "starting");
        assert_eq!(health["ready"], false);
        assert_eq!(health["control_enabled"], false);

        for uri in [
            "/v1/project",
            "/v1/cues",
            "/v1/cues/active",
            "/v1/status",
            "/v1/status/history?seconds=300",
            "/v1/logs?after=0",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
        }

        let response = app
            .clone()
            .oneshot(Request::get("/v1/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");

        let response = app
            .oneshot(
                Request::get("/v1/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let document = json(response).await;
        assert!(document["paths"]["/v1/status"].is_object());
        assert!(document["paths"]["/v1/commands"].is_object());
        assert_eq!(
            document["components"]["securitySchemes"]["bearer_token"]["scheme"],
            "bearer"
        );
        assert_eq!(
            document["paths"]["/v1/commands"]["post"]["security"][0]["bearer_token"],
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn control_requires_configuration_and_a_valid_token() {
        let body = r#"{"command":"go"}"#;
        let (state, _) = test_api(None);
        let response = build_router(state)
            .oneshot(
                Request::post("/v1/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(json(response).await["error"], "control_disabled");

        let (state, _) = test_api(Some("secret"));
        let response = build_router(state)
            .oneshot(
                Request::post("/v1/commands")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, "Bearer wrong")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[WWW_AUTHENTICATE], "Bearer");
    }

    #[tokio::test]
    async fn command_result_moves_from_pending_to_applied() {
        let (state, mut command_rx) = test_api(Some("secret"));
        state.ready.store(true, Ordering::Release);
        let app = build_router(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/commands")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, "Bearer secret")
                    .body(Body::from(r#"{"command":"select_cue","qid":"1.5"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let accepted = json(response).await;
        let id = accepted["id"].as_u64().unwrap();
        assert_eq!(accepted["state"], "pending");

        let request = command_rx.try_recv().unwrap();
        assert_eq!(request.id, id);
        assert!(matches!(request.command, ApiCommand::SelectCue { qid } if qid == "1.5"));
        state.complete_command(ApiCommandResult {
            id,
            outcome: ApiCommandOutcome::Applied("cue selected".into()),
        });

        let response = app
            .oneshot(
                Request::get(format!("/v1/commands/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let completed = json(response).await;
        assert_eq!(completed["state"], "applied");
        assert_eq!(completed["message"], "cue selected");
    }

    #[tokio::test]
    async fn idempotency_key_prevents_duplicate_commands() {
        let (state, mut command_rx) = test_api(Some("secret"));
        state.ready.store(true, Ordering::Release);
        let app = build_router(state);
        let request = || {
            Request::post("/v1/commands")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, "Bearer secret")
                .header("idempotency-key", "operator-42")
                .body(Body::from(r#"{"command":"go"}"#))
                .unwrap()
        };

        let first = app.clone().oneshot(request()).await.unwrap();
        let second = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        assert_eq!(second.status(), StatusCode::ACCEPTED);
        assert_eq!(json(first).await["id"], json(second).await["id"]);
        command_rx.try_recv().unwrap();
        assert!(command_rx.try_recv().is_err());

        let conflict = app
            .oneshot(
                Request::post("/v1/commands")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, "Bearer secret")
                    .header("idempotency-key", "operator-42")
                    .body(Body::from(r#"{"command":"stop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn expired_idempotency_keys_fail_closed() {
        let (state, mut command_rx) = test_api(Some("secret"));
        let first = state
            .register_command(ApiCommand::Go, None, Some("original".into()))
            .unwrap();
        command_rx.try_recv().unwrap();
        state.complete_command(ApiCommandResult {
            id: first.id,
            outcome: ApiCommandOutcome::Applied("done".into()),
        });
        for _ in 0..COMMAND_HISTORY_CAPACITY {
            let command = state.register_command(ApiCommand::Stop, None, None).unwrap();
            command_rx.try_recv().unwrap();
            state.complete_command(ApiCommandResult {
                id: command.id,
                outcome: ApiCommandOutcome::Applied("done".into()),
            });
        }

        assert!(matches!(
            state.idempotent_command("original", &ApiCommand::Go),
            Err(error) if error.status == StatusCode::CONFLICT
        ));
    }

    #[test]
    fn direct_network_binding_is_rejected() {
        assert!(validate_bind_address("127.0.0.1:7133".parse().unwrap()).is_ok());
        assert!(validate_bind_address("[::1]:7133".parse().unwrap()).is_ok());
        assert!(validate_bind_address("0.0.0.0:7133".parse().unwrap()).is_err());
    }
}
