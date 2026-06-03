//! Standalone API server — runs on a background tokio thread.

use crate::openapi::ApiDoc;
use crate::SharedState;
use axum::{extract::Extension, middleware, response::IntoResponse, Router};
use std::sync::{Arc, Mutex};
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Configuration for the API server.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Host to bind to (default: "0.0.0.0").
    pub host: String,
    /// Port to listen on.
    pub port: u16,
    /// When true, clients on the same LAN subnet skip token auth.
    pub lan_trust: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8082,
            lan_trust: false,
        }
    }
}

/// Handle for gracefully shutting down the API server.
pub struct ApiServerHandle {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl ApiServerHandle {
    /// Signal shutdown and wait for the thread to finish.
    pub fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::warn!("API server thread panicked: {:?}", e);
            }
        }
    }
}

impl Drop for ApiServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Web server state used by the API server's auth middleware.
/// This is a minimal struct that mirrors rustjay-control's WebServerState
/// just enough for auth.
pub(crate) struct ApiServerState {
    pub(crate) bearer_token: String,
    pub(crate) lan_trust: bool,
}

/// Start the HTTP API server on a background tokio runtime.
///
/// Returns an `ApiServerHandle` for graceful shutdown, or `None` if binding failed.
pub fn start(config: ApiConfig, shared: SharedState) -> Option<ApiServerHandle> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let token = generate_token();
    let lan_trust = config.lan_trust;

    let auth_state = Arc::new(Mutex::new(ApiServerState {
        bearer_token: token.clone(),
        lan_trust,
    }));

    let app = build_router(shared, auth_state);

    let addr: std::net::SocketAddr = match format!("{}:{}", config.host, config.port).parse() {
        Ok(a) => a,
        Err(e) => {
            log::error!("Invalid API bind address {}:{}: {}", config.host, config.port, e);
            return None;
        }
    };

    let thread_handle = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("Failed to create tokio runtime for API server: {}", e);
                    return;
                }
            };

            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        log::error!("Failed to bind API server on {}: {}", addr, e);
                        return;
                    }
                };
                let local_addr = listener.local_addr().unwrap();
                log::info!("API server listening on http://{}", local_addr);
                log::info!("  Swagger UI: http://{}/swagger-ui", local_addr);
                log::info!("  API token: {}", token);

                let mut shutdown_rx = shutdown_rx;
                if let Err(e) = axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.wait_for(|&v| v).await;
                        log::info!("API server shutting down...");
                    })
                    .await
                {
                    log::error!("API server error: {}", e);
                }
                log::info!("API server stopped");
            });
        }));
        if result.is_err() {
            log::error!("API server thread panicked");
        }
    });

    Some(ApiServerHandle { shutdown_tx, thread_handle: Some(thread_handle) })
}

/// Build the axum router with all routes, auth, and OpenAPI.
pub(crate) fn build_router(shared: SharedState, auth_state: Arc<Mutex<ApiServerState>>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let protected = crate::build_router(shared)
        .layer(middleware::from_fn(auth_middleware))
        .layer(Extension(auth_state));

    Router::new()
        .merge(protected)
        .merge(SwaggerUi::new("/swagger-ui").url("/api/openapi.json", ApiDoc::openapi()))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .layer(cors)
}

/// Bearer-token auth middleware for the API server.
async fn auth_middleware(
    Extension(state): Extension<Arc<Mutex<ApiServerState>>>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
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
            req.uri()
                .query()
                .map(|q| q.contains(&format!("token={}", token)))
                .unwrap_or(false)
        }
    };

    if auth_ok {
        next.run(req).await
    } else {
        axum::http::StatusCode::UNAUTHORIZED.into_response()
    }
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
