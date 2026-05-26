//! # Web Remote Control Interface
//!
//! WebSocket-based web interface for remote control from phones/tablets.
//! URL: http://[computer-ip]:[port]/[app_name]

use axum::{
    extract::{ws::{WebSocket, Message}, State, WebSocketUpgrade, Query},
    response::IntoResponse,
    routing::get,
    Router, middleware::{self, Next},
    http::{Request, StatusCode, HeaderMap},
    body::Body,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Commands for web server lifecycle control
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebControlCommand {
    None,
    Start,
    Stop,
    SetPort(u16),
}

/// Web server configuration
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Host to bind to (default: 0.0.0.0 — all interfaces)
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// App name for URL path (e.g., "rustjay")
    pub app_name: String,
    /// Whether server is running
    pub enabled: bool,
    /// When true, clients on the same LAN subnet connect without a token.
    pub lan_trust: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8081,
            app_name: "rustjay-template".to_string(),
            enabled: false,
            lan_trust: false,
        }
    }
}

/// Parameter definition for web UI
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebParameter {
    pub id: String,
    pub name: String,
    pub category: String,
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub step: f32,
    pub options: Option<Vec<String>>,
}

/// Web server state shared between handlers
/// Shared state for the web server.
pub struct WebServerState {
    /// Server configuration.
    pub config: WebConfig,
    /// All available parameters.
    pub parameters: HashMap<String, WebParameter>,
    /// Channel for broadcasting updates to all connected clients.
    pub broadcast_tx: broadcast::Sender<WebMessage>,
    /// Channel for receiving updates from clients.
    pub command_tx: tokio::sync::mpsc::Sender<WebCommand>,
    /// Per-launch bearer token for auth.
    pub bearer_token: String,
    /// When true, clients on the same LAN subnet skip token auth.
    pub lan_trust: bool,
}

/// Messages sent from server to web clients
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum WebMessage {
    #[serde(rename = "params")]
    Params { params: Vec<WebParameter> },
    #[serde(rename = "update")]
    Update { id: String, value: f32 },
    #[serde(rename = "connected")]
    Connected { client_count: usize },
}

/// Commands received from web clients
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type")]
/// Commands received from web clients.
pub enum WebCommand {
    /// Set a parameter value.
    #[serde(rename = "set")]
    Set {
        /// Parameter identifier.
        id: String,
        /// New value.
        value: f32,
    },
}

/// Web server handle.
pub struct WebServer {
    /// Shared server state.
    pub state: Arc<Mutex<WebServerState>>,
    /// Channel receiving commands from web clients.
    pub command_rx: tokio::sync::mpsc::Receiver<WebCommand>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Last value broadcast for each parameter.  Used for diff-tracking so
    /// unchanged parameters skip the mutex entirely.
    last_sent: HashMap<String, f32>,
}

impl WebServer {
    /// Create a new web server and its command channel.
    pub fn new(config: WebConfig) -> (Self, tokio::sync::mpsc::Sender<WebCommand>) {
        let (broadcast_tx, _) = broadcast::channel(100);
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);

        let bearer_token = generate_token();
        let lan_trust = config.lan_trust;

        let state = Arc::new(Mutex::new(WebServerState {
            config,
            parameters: HashMap::new(),
            broadcast_tx,
            command_tx: command_tx.clone(),
            bearer_token,
            lan_trust,
        }));

        let server = Self {
            state,
            command_rx,
            handle: None,
            shutdown_tx: None,
            last_sent: HashMap::new(),
        };

        (server, command_tx)
    }

    /// Register a parameter for the web UI
    pub fn register_parameter(&mut self, id: &str, name: &str, category: &str, min: f32, max: f32, value: f32, step: f32) {
        // Clear stale diff-tracking entry so the initial broadcast is never skipped.
        self.last_sent.remove(id);
        if let Ok(mut state) = self.state.lock() {
            state.parameters.insert(id.to_string(), WebParameter {
                id: id.to_string(),
                name: name.to_string(),
                category: category.to_string(),
                min,
                max,
                value,
                step,
                options: None,
            });
        }
    }

    /// Register an enum parameter for the web UI (rendered as a select/dropdown)
    pub fn register_enum_parameter(&mut self, id: &str, name: &str, category: &str, options: Vec<String>, value: f32) {
        self.last_sent.remove(id);
        if let Ok(mut state) = self.state.lock() {
            state.parameters.insert(id.to_string(), WebParameter {
                id: id.to_string(),
                name: name.to_string(),
                category: category.to_string(),
                min: 0.0,
                max: (options.len() as f32) - 1.0,
                value,
                step: 1.0,
                options: Some(options),
            });
        }
    }

    /// Register default parameters (color, audio, etc.)
    pub fn register_default_parameters(&mut self) {
        // Color parameters
        self.register_parameter("color/hue_shift", "Hue Shift", "Color", -180.0, 180.0, 0.0, 1.0);
        self.register_parameter("color/saturation", "Saturation", "Color", 0.0, 2.0, 1.0, 0.01);
        self.register_parameter("color/brightness", "Brightness", "Color", 0.0, 2.0, 1.0, 0.01);
        self.register_parameter("color/enabled", "Color Enabled", "Color", 0.0, 1.0, 1.0, 1.0);

        // Audio parameters
        self.register_parameter("audio/amplitude", "Amplitude", "Audio", 0.0, 5.0, 1.0, 0.01);
        self.register_parameter("audio/smoothing", "Smoothing", "Audio", 0.0, 1.0, 0.5, 0.01);
        self.register_parameter("audio/enabled", "Audio Enabled", "Audio", 0.0, 1.0, 1.0, 1.0);
        self.register_parameter("audio/normalize", "Normalize", "Audio", 0.0, 1.0, 1.0, 1.0);
        self.register_parameter("audio/pink_noise", "Pink Noise", "Audio", 0.0, 1.0, 0.0, 1.0);

        // Output parameters
        self.register_parameter("output/fullscreen", "Fullscreen", "Output", 0.0, 1.0, 0.0, 1.0);
    }

    /// Register effect-declared parameters dynamically.
    pub fn register_parameters(&mut self, descriptors: &[rustjay_core::ParameterDescriptor]) {
        for d in descriptors {
            let category = d.category.name();
            let id = format!("{}/{}", category.to_lowercase(), d.id);
            match &d.param_type {
                rustjay_core::ParamType::Enum { variants } => {
                    self.register_enum_parameter(&id, &d.name, &category, variants.clone(), d.default);
                }
                _ => {
                    self.register_parameter(&id, &d.name, &category, d.min, d.max, d.default, d.step);
                }
            }
        }
    }

    /// Update a parameter value and broadcast to all clients.
    ///
    /// Uses a fast-path `last_sent` cache so unchanged values skip the
    /// state mutex entirely — this removes ~N mutex acquisitions per frame
    /// where N is the number of registered parameters.
    pub fn update_parameter(&mut self, id: &str, value: f32) {
        const THRESHOLD: f32 = 0.001;

        // NaN/inf would loop forever (abs diff always false); reject at boundary.
        if !value.is_finite() {
            return;
        }

        // Fast path: if we already sent this value, do nothing.
        if let Some(&last) = self.last_sent.get(id) {
            if (value - last).abs() < THRESHOLD {
                return;
            }
        }

        let mut should_broadcast = false;

        if let Ok(mut state) = self.state.lock() {
            if let Some(param) = state.parameters.get_mut(id) {
                // Only update if changed
                if (param.value - value).abs() > 0.0001 {
                    param.value = value;
                    should_broadcast = true;
                }
            }
        }

        if should_broadcast {
            self.last_sent.insert(id.to_string(), value);
            if let Ok(state) = self.state.lock() {
                let _ = state.broadcast_tx.send(WebMessage::Update {
                    id: id.to_string(),
                    value,
                });
            }
        }
    }

    /// Start the web server (creates its own tokio runtime)
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.handle.is_some() {
            return Ok(()); // Already running
        }

        let state = Arc::clone(&self.state);
        let (port, app_name, host, token) = {
            let s = state.lock().unwrap_or_else(|e| e.into_inner());
            (s.config.port, s.config.app_name.clone(), s.config.host.clone(), s.bearer_token.clone())
        };

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let handle = std::thread::spawn(move || {
            // Create a new tokio runtime for this thread
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("Failed to create tokio runtime: {}", e);
                    return;
                }
            };

            rt.block_on(async move {
                let app = create_router(state, &app_name, &token);

                let addr: SocketAddr = match format!("{}:{}", host, port).parse() {
                    Ok(a) => a,
                    Err(e) => {
                        log::error!("Invalid web server bind address {}:{}: {}", host, port, e);
                        return;
                    }
                };

                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        log::info!("Web server bound to {}", addr);
                        l
                    }
                    Err(e) => {
                        log::error!("Failed to bind web server to {}: {}", addr, e);
                        return;
                    }
                };

                let local_ip = get_local_ip().unwrap_or_else(|| "localhost".to_string());
                log::info!("Web server ready:");
                log::info!("  Local:   http://127.0.0.1:{}/{}?token={}", port, app_name, token);
                if host != "127.0.0.1" && host != "localhost" {
                    log::info!("  Network: http://{}:{}/{}?token={}", local_ip, port, app_name, token);
                }

                // Run server with graceful shutdown
                let server = axum::serve(listener, app);

                tokio::select! {
                    result = server => {
                        if let Err(e) = result {
                            log::error!("Web server error: {}", e);
                        }
                    }
                    _ = shutdown_rx => {
                        log::info!("Web server received shutdown signal");
                    }
                }
            });
        });

        self.handle = Some(handle);

        // Update config
        if let Ok(mut state) = self.state.lock() {
            state.config.enabled = true;
        }

        Ok(())
    }

    /// Stop the web server
    pub fn stop(&mut self) {
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for thread to finish
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            log::info!("Web server stopped");
        }

        // Update config
        if let Ok(mut state) = self.state.lock() {
            state.config.enabled = false;
        }
    }

    /// Check if server is running
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Get the server URL (no token)
    pub fn get_url(&self) -> String {
        if let Ok(state) = self.state.lock() {
            format!("http://{}:{}/{}",
                state.config.host,
                state.config.port,
                state.config.app_name
            )
        } else {
            String::new()
        }
    }

    /// Get the bearer token.
    pub fn get_token(&self) -> String {
        self.state.lock()
            .map(|s| s.bearer_token.clone())
            .unwrap_or_default()
    }

    /// Get the full access URL including the auth token, using the actual local IP.
    pub fn get_full_url(&self) -> String {
        if let Ok(state) = self.state.lock() {
            let ip = get_local_ip().unwrap_or_else(|| "localhost".to_string());
            format!("http://{}:{}/{}?token={}",
                ip,
                state.config.port,
                state.config.app_name,
                state.bearer_token,
            )
        } else {
            String::new()
        }
    }

    /// Live-update the LAN trust setting without restarting the server.
    pub fn set_lan_trust(&self, enabled: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.lan_trust = enabled;
        }
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Create the Axum router
fn create_router(state: Arc<Mutex<WebServerState>>, app_name: &str, token: &str) -> Router {
    let ws_path = format!("/{}/ws", app_name);
    let page_path = format!("/{}", app_name);
    let page_path_slash = format!("/{}/", app_name);
    let page_path_redirect = page_path.clone();
    let page_path_redirect2 = page_path_redirect.clone();

    let html_with_token = inject_token_into_html(EMBEDDED_HTML, token, app_name);

    // Protected routes: auth required for everything except /health.
    // Auth middleware receives the shared state so it can read `lan_trust` live.
    let protected = Router::new()
        .route(&ws_path, get(ws_handler))
        .route(&page_path, get(move || async move {
            index_handler(&html_with_token).await
        }))
        .route(&page_path_slash, get(move || async move {
            axum::response::Redirect::permanent(&page_path_redirect)
        }))
        .route("/", get(move || async move {
            axum::response::Redirect::temporary(&page_path_redirect2)
        }))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth_middleware,
        ));

    Router::new()
        .route("/health", get(|| async { "OK" }))
        .merge(protected)
        .with_state(state)
}

/// Injects the bearer token and app name into the HTML.
fn inject_token_into_html(html: &str, token: &str, app_name: &str) -> String {
    let script = format!(r#"<script>window.RUSTJAY_TOKEN = "{}";</script>"#, token);
    let html = html.replacen("<head>", &format!("<head>{}", script), 1);
    html.replace("__APP__", &app_name.to_uppercase())
}

/// Response with proper content type for HTML
async fn index_handler(html: &str) -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CONNECTION, "keep-alive"),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; style-src 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss: http: https:",
            ),
        ],
        html.to_string()
    )
}

/// Query parameters for WebSocket upgrade
#[derive(Debug, serde::Deserialize)]
struct WsQuery {
    token: String,
}

/// WebSocket handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<Mutex<WebServerState>>>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Verify bearer token from query param (browsers can't set custom headers on WebSocket).
    // Skip when lan_trust is enabled.
    let (valid_token, lan_trust) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        (s.bearer_token.clone(), s.lan_trust)
    };
    if !lan_trust && query.token != valid_token {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Verify Origin header is present and non-empty (browser WebSocket requirement).
    // Requests without an Origin header are rejected to prevent curl/scripts from
    // bypassing origin checks.
    match headers.get(axum::http::header::ORIGIN) {
        Some(origin) => {
            let origin_str = origin.to_str().unwrap_or("");
            if origin_str.is_empty() {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
        None => {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle a WebSocket connection
async fn handle_socket(mut socket: WebSocket, state: Arc<Mutex<WebServerState>>) {
    // Get initial parameters
    let params = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        state.parameters.values().cloned().collect::<Vec<_>>()
    };

    // Send initial params list
    let init_msg = WebMessage::Params { params };
    if let Ok(json) = serde_json::to_string(&init_msg) {
        if socket.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }

    // Subscribe to broadcasts
    let mut rx = {
        state.lock().unwrap_or_else(|e| e.into_inner()).broadcast_tx.subscribe()
    };

    // Handle messages from client and broadcasts
    loop {
        tokio::select! {
            // Receive broadcast from server
            Ok(msg) = rx.recv() => {
                if let Ok(json) = serde_json::to_string(&msg) {
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break; // Client disconnected
                    }
                }
            }
            // Receive message from client
            Some(Ok(msg)) = socket.recv() => {
                if let Message::Text(text) = msg {
                    if let Ok(cmd) = serde_json::from_str::<WebCommand>(&text) {
                        match &cmd {
                            WebCommand::Set { id, value } => {
                                let id = id.clone();
                                let value = *value;

                                // Update local state and broadcast to other clients
                                let mut should_broadcast = false;
                                if let Ok(mut state) = state.lock() {
                                    if let Some(param) = state.parameters.get_mut(&id) {
                                        if (param.value - value).abs() > 0.0001 {
                                            param.value = value;
                                            should_broadcast = true;
                                        }
                                    }
                                    // Forward command to app
                                    let _ = state.command_tx.try_send(cmd);
                                }

                                if should_broadcast {
                                    if let Ok(state) = state.lock() {
                                        let _ = state.broadcast_tx.send(WebMessage::Update {
                                            id,
                                            value,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            else => break,
        }
    }
}

/// Bearer-token auth middleware.
/// Accepts the token via `Authorization: Bearer <token>` header or `?token=<token>` query param.
/// When `lan_trust` is enabled in server state, all requests pass through without a token.
async fn auth_middleware(
    State(state): State<Arc<Mutex<WebServerState>>>,
    req: Request<Body>,
    next: Next,
) -> impl IntoResponse {
    let (token, lan_trust) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        (s.bearer_token.clone(), s.lan_trust)
    };

    if lan_trust {
        return next.run(req).await;
    }

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let auth_ok = match auth_header {
        Some(header) if header == format!("Bearer {}", token) => true,
        _ => {
            // Allow token via query parameter so the HTML page can be accessed
            // directly in a browser (e.g. http://host:port/app_name?token=xxx).
            req.uri()
                .query()
                .map(|q| q.contains(&format!("token={}", token)))
                .unwrap_or(false)
        }
    };

    if auth_ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Get local IP address
fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    // Try to connect to a public DNS server to determine local IP
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return Some(addr.ip().to_string());
            }
        }
    }
    None
}

/// Generate a random 16-byte hex token.
fn generate_token() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
        s
    })
}

/// Embedded HTML/JS/CSS for the web UI
const EMBEDDED_HTML: &str = include_str!("ui.html");
