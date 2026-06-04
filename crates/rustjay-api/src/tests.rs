//! Integration tests for rustjay-api.
//!
//! These exercise the router produced by [`build_router`]. The router is
//! stateless in the type sense, so the tests supply state via `.with_state(...)`
//! to obtain a serveable `Router<()>`. Auth is supplied by `rustjay-control`
//! when the router is merged under its protected tree, so it is not covered
//! here — these tests hit the routes directly.

use crate::build_router;
use axum::body::Body;
use axum::Router;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

/// Create a `WebServer` whose shared state backs the API router. The server is
/// returned so the caller can keep it alive: it owns the command-channel
/// receiver, which must outlive the test for `try_send` to succeed.
fn test_server() -> rustjay_control::WebServer {
    let (server, _command_tx) =
        rustjay_control::WebServer::new(rustjay_control::WebConfig::default());
    server
}

#[tokio::test]
async fn test_build_router_does_not_panic() {
    let server = test_server();
    let _router: Router<()> = build_router().with_state(Arc::clone(&server.state));
}

#[tokio::test]
async fn test_health_returns_ok() {
    let server = test_server();
    let app = build_router().with_state(Arc::clone(&server.state));

    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(status == StatusCode::OK, "Expected 200, got {status}. Body: {body_str}");
}

#[tokio::test]
async fn test_state_returns_503_when_not_initialized() {
    let server = test_server();
    let app = build_router().with_state(Arc::clone(&server.state));

    let resp = app
        .oneshot(Request::get("/api/state").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_set_param_returns_ok() {
    let server = test_server();
    let app = build_router().with_state(Arc::clone(&server.state));

    let body = serde_json::json!({"id": "color/hue_shift", "value": 0.5});
    let resp = app
        .oneshot(
            Request::put("/api/params")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}
