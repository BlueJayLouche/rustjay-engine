//! Integration tests for rustjay-api.

use crate::{build_router, SharedState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

fn test_shared_state() -> SharedState {
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    // Keep the receiver alive so try_send succeeds in tests.
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let snapshot = Arc::new(RwLock::new(None));
    SharedState {
        command_tx: tx,
        engine_snapshot: snapshot,
    }
}

#[tokio::test]
async fn test_build_router_does_not_panic() {
    let shared = test_shared_state();
    let _router = build_router(shared);
}

#[tokio::test]
async fn test_health_returns_ok() {
    let shared = test_shared_state();
    let app = build_router(shared);

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
    let shared = test_shared_state();
    let app = build_router(shared);

    let resp = app
        .oneshot(Request::get("/api/state").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_set_param_returns_ok() {
    let shared = test_shared_state();
    let app = build_router(shared);

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

#[tokio::test]
async fn test_auth_rejects_without_token() {
    let shared = test_shared_state();
    let auth_state = Arc::new(std::sync::Mutex::new(crate::server::ApiServerState {
        bearer_token: "test-token-123".to_string(),
        lan_trust: false,
    }));

    let app = crate::server::build_router(shared, auth_state);

    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // Health is inside the protected router, so auth applies.
    // Wait — in our build_router, /api/health is NOT inside the protected router
    // that has auth. Let me check...
    // Actually, build_router in lib.rs has /api/health directly, without auth.
    // The auth is only added in server.rs's build_router.
    // So this test tests the server's build_router.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_accepts_with_token() {
    let shared = test_shared_state();
    let auth_state = Arc::new(std::sync::Mutex::new(crate::server::ApiServerState {
        bearer_token: "test-token-123".to_string(),
        lan_trust: false,
    }));

    let app = crate::server::build_router(shared, auth_state);

    let resp = app
        .oneshot(
            Request::get("/api/health")
                .header("Authorization", "Bearer test-token-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_skips_when_lan_trust() {
    let shared = test_shared_state();
    let auth_state = Arc::new(std::sync::Mutex::new(crate::server::ApiServerState {
        bearer_token: "test-token-123".to_string(),
        lan_trust: true,
    }));

    let app = crate::server::build_router(shared, auth_state);

    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}
