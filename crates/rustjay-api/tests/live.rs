//! Live verification: bind the *real* consolidated server (control web server
//! with the rustjay-api router merged under its auth layer) on a real TCP port
//! and exercise it over actual sockets. Dependency-free — raw HTTP over
//! `std::net::TcpStream`. The token/Origin/connection-cap checks all resolve to
//! an HTTP status before the WebSocket upgrade, so they are observable here
//! without a full WS client.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustjay_api::build_router;
use rustjay_control::{WebConfig, WebServer};

/// A minimal HTTP response: status code + body.
struct Resp {
    status: u16,
    body: String,
}

/// Send one raw HTTP/1.1 request to 127.0.0.1:port and read the full response.
fn http(port: u16, request: &str) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(request.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                // Servers using keep-alive won't close; stop once we have headers
                // plus any body that fits in the first reads. For these small
                // responses a single read is enough; guard with a length check.
                if raw.windows(4).any(|w| w == b"\r\n\r\n") && raw.len() < 4096 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Resp { status, body }
}

/// Pick a free port by binding to :0 and releasing it.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Start the real consolidated server; return (port, token).
fn start_server() -> (u16, String) {
    let port = free_port();
    let config = WebConfig {
        host: "127.0.0.1".to_string(),
        port,
        app_name: "test".to_string(),
        enabled: false,
        lan_trust: false,
        token: None,
    };
    let (mut server, _tx) = WebServer::new(config);
    server.set_api_router(build_router());
    let engine = Arc::new(Mutex::new(rustjay_core::EngineState::new()));
    server.set_engine_state(engine);
    server.start().expect("server start");
    let token = server.get_token();
    // Keep the server alive for the whole test process.
    std::mem::forget(server);

    // Wait for the listener to accept connections.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    (port, token)
}

#[test]
fn live_consolidated_server() {
    let (port, token) = start_server();

    // 1. Single listener: nothing on port+1 (the old second-server port).
    assert!(
        TcpStream::connect(("127.0.0.1", port + 1)).is_err(),
        "port+1 must not be bound — there should be exactly one listener"
    );

    // 2. /api/state requires auth (regression: it was unauthenticated when the
    //    api router was merged after route_layer).
    let r = http(
        port,
        "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(r.status, 401, "GET /api/state without token must be 401");

    // 3. Swagger UI is behind auth too.
    let r = http(
        port,
        "GET /swagger-ui/ HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(r.status, 401, "/swagger-ui without token must be 401");

    // 4. REST read with token → 200, and the body is the engine snapshot JSON.
    let req = format!(
        "GET /api/state?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    let r = http(port, &req);
    assert_eq!(r.status, 200, "GET /api/state with token must be 200");
    assert!(
        r.body.contains("\"performance\""),
        "snapshot JSON expected, got: {}",
        r.body
    );

    // 5. REST write round-trip: PUT /api/params with token → 200.
    let body = r#"{"id":"color/hue_shift","value":0.5}"#;
    let req = format!(
        "PUT /api/params?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len()
    );
    let r = http(port, &req);
    assert_eq!(
        r.status, 200,
        "PUT /api/params with token must be 200; body: {}",
        r.body
    );

    // 6. WS upgrade with a valid token but a foreign Origin → 403 (Origin check).
    let req = format!(
        "GET /test/ws?token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    );
    let r = http(port, &req);
    assert_eq!(r.status, 403, "WS upgrade with foreign Origin must be 403");

    // 7. WS upgrade with no token → 401 (auth before upgrade).
    let req = format!(
        "GET /test/ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    );
    let r = http(port, &req);
    assert_eq!(r.status, 401, "WS upgrade without token must be 401");

    // 8. Fixed-token server also returns 401 without the token.
    assert_fixed_token_401();
}

/// Start a server with a fixed bearer token and verify 401 without it.
/// This is appended to `live_consolidated_server` to avoid port races
/// when multiple live tests run in parallel.
fn assert_fixed_token_401() {
    let port = free_port();
    let config = WebConfig {
        host: "127.0.0.1".to_string(),
        port,
        app_name: "test".to_string(),
        enabled: false,
        lan_trust: false,
        token: Some("my-fixed-token".to_string()),
    };
    let (mut server, _tx) = WebServer::new(config);
    server.set_api_router(build_router());
    let engine = Arc::new(Mutex::new(rustjay_core::EngineState::new()));
    server.set_engine_state(engine);
    server.start().expect("server start");
    // Keep the server alive for the whole test process.
    std::mem::forget(server);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Without the fixed token → 401.
    let r = http(
        port,
        "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        r.status, 401,
        "GET /api/state without fixed token must be 401"
    );

    // With the fixed token → 200.
    let req = "GET /api/state?token=my-fixed-token HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".to_string();
    let r = http(port, &req);
    assert_eq!(r.status, 200, "GET /api/state with fixed token must be 200");
}
