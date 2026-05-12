//! # Web Remote Control Interface
//!
//! WebSocket-based web interface for remote control from phones/tablets.
//! URL: http://[computer-ip]:[port]/[app_name]

use axum::{
    extract::{ws::{WebSocket, Message}, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

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
    /// Port to listen on
    pub port: u16,
    /// App name for URL path (e.g., "rustjay")
    pub app_name: String,
    /// Whether server is running
    pub enabled: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            port: 8081,
            app_name: "rustjay-template".to_string(),
            enabled: false,
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
pub struct WebServerState {
    pub config: WebConfig,
    /// All available parameters
    pub parameters: HashMap<String, WebParameter>,
    /// Channel for broadcasting updates to all connected clients
    pub broadcast_tx: broadcast::Sender<WebMessage>,
    /// Channel for receiving updates from clients
    pub command_tx: tokio::sync::mpsc::Sender<WebCommand>,
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
pub enum WebCommand {
    #[serde(rename = "set")]
    Set { id: String, value: f32 },
}

/// Web server handle
pub struct WebServer {
    pub state: Arc<Mutex<WebServerState>>,
    pub command_rx: tokio::sync::mpsc::Receiver<WebCommand>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebServer {
    pub fn new(config: WebConfig) -> (Self, tokio::sync::mpsc::Sender<WebCommand>) {
        let (broadcast_tx, _) = broadcast::channel(100);
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);
        
        let state = Arc::new(Mutex::new(WebServerState {
            config,
            parameters: HashMap::new(),
            broadcast_tx,
            command_tx: command_tx.clone(),
        }));
        
        let server = Self {
            state,
            command_rx,
            handle: None,
            shutdown_tx: None,
        };
        
        (server, command_tx)
    }
    
    /// Register a parameter for the web UI
    pub fn register_parameter(&mut self, id: &str, name: &str, category: &str, min: f32, max: f32, value: f32, step: f32) {
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
    
    /// Update a parameter value and broadcast to all clients
    pub fn update_parameter(&mut self, id: &str, value: f32) {
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
        let (port, app_name) = {
            let s = state.lock().unwrap_or_else(|e| e.into_inner());
            (s.config.port, s.config.app_name.clone())
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
                let app = create_router(state, &app_name);
                
                // Try binding to all interfaces first, then fallback to localhost
                let addr = SocketAddr::from(([0, 0, 0, 0], port));
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        log::info!("Web server bound to all interfaces (0.0.0.0):{}", port);
                        l
                    }
                    Err(e) => {
                        log::warn!("Failed to bind to all interfaces: {}. Trying localhost...", e);
                        let local_addr = SocketAddr::from(([127, 0, 0, 1], port));
                        match tokio::net::TcpListener::bind(local_addr).await {
                            Ok(l) => {
                                log::info!("Web server bound to localhost:{}", port);
                                l
                            }
                            Err(e2) => {
                                log::error!("Failed to bind web server to {} or {}: {} / {}", addr, local_addr, e, e2);
                                return;
                            }
                        }
                    }
                };
                
                let local_ip = get_local_ip().unwrap_or_else(|| "localhost".to_string());
                log::info!("Web server ready:");
                log::info!("  Local:   http://127.0.0.1:{}/{}", port, app_name);
                log::info!("  Network: http://{}:{}/{}", local_ip, port, app_name);
                
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
    
    /// Get the server URL
    pub fn get_url(&self) -> String {
        if let Ok(state) = self.state.lock() {
            format!("http://{}:{}/{}",
                get_local_ip().unwrap_or_else(|| "localhost".to_string()),
                state.config.port,
                state.config.app_name
            )
        } else {
            String::new()
        }
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Create the Axum router
fn create_router(state: Arc<Mutex<WebServerState>>, app_name: &str) -> Router {
    let ws_path = format!("/{}/ws", app_name);
    let page_path = format!("/{}", app_name);
    
    Router::new()
        .route(&ws_path, get(ws_handler))
        .route(&page_path, get(index_handler))
        .route("/", get(move || async move { 
            axum::response::Redirect::temporary(&page_path) 
        }))
        .route("/health", get(|| async { "OK" }))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Response with proper content type for HTML
async fn index_handler() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CONNECTION, "keep-alive"),
        ],
        EMBEDDED_HTML
    )
}



/// WebSocket handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<Mutex<WebServerState>>>,
) -> impl IntoResponse {
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

/// Embedded HTML/JS/CSS for the web UI
const EMBEDDED_HTML: &str = include_str!("ui.html");
