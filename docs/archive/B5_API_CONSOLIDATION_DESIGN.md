# B5 API Consolidation Design — rustjay-api onto rustjay-control

**Branch:** `feat/b5-rustjay-api`  
**Goal:** Eliminate the parallel tokio runtime/listener in `rustjay-api`; mount its typed REST + OpenAPI/Swagger on the **existing** `rustjay-control` server.

---

## 1. Root Cause

`rustjay-api` currently:
- Spawns its own `std::thread` + `tokio::runtime::Builder::new_multi_thread()` (`server.rs:88-115`)
- Binds its own `TcpListener` on `port+1` (`app/mod.rs:340`)
- Reimplements auth (`server.rs:154-188`) with non-constant-time compare
- Deep-clones `EngineSnapshot` every frame even with zero HTTP clients (`update.rs:516-520`)
- Serves Swagger UI **outside** auth (`server.rs:148`)
- Uses `CorsLayer::permissive()` + `Any` (`lib.rs:31-34`)
- Runs a custom `/api/ws` that re-serializes + JSON-Patch diffs per client (`ws.rs:87-98`)

All of this duplicates what `rustjay-control` already does.

---

## 2. Composition Mechanism (Chosen)

**Approach B — `WebServer::set_api_router(router)` hook**

`rustjay-control` adds a single method to `WebServer`:

```rust
impl WebServer {
    /// Merge an external router (e.g. REST/OpenAPI) into the protected
    /// router so it shares the same auth layer and listener.
    pub fn set_api_router(&mut self, router: axum::Router) { ... }
}
```

- `rustjay-api` exports **only** `build_router(shared: SharedState) -> Router` + `SharedState`.
- It owns **no** runtime, thread, or listener.
- The engine (`app/mod.rs` and `gles2.rs`) is the composition root:
  1. Creates `WebServer` (as today)
  2. Calls `rustjay_api::build_router(...)` to get the API router
  3. Calls `web_server.set_api_router(api_router)`
  4. Calls `web_server.start()` — **one** runtime, **one** listener

**Why not expose `create_router` + auth layer publicly?**
- `create_router` is an internal detail; leaking it commits to its signature.
- A `set_api_router` hook is 3 lines, keeps `create_router` private, and is zero-cost when unused.
- Apps without the `api` feature compile exactly as before (the field is `Option<Router>`, default `None`).

---

## 3. Per-Crate Changes

### `rustjay-control` (host)
- Add `api_router: Option<Router>` to `WebServer`
- Add `pub fn set_api_router(&mut self, router: Router)`
- In `create_router`, merge `api_router` into `protected` **before** `.route_layer(auth_middleware)` so API routes inherit auth automatically. (`route_layer` only wraps routes already present; merging *after* it leaves `/api/*` and `/swagger-ui` unauthenticated.)
- **Security hardening in `auth_middleware`** (Fixes #16 for both control and API):
  - Replace `header == format!("Bearer {}", token)` with constant-time `subtle`-style byte compare (no new dep)
  - Replace `q.contains(&format!("token={}", token))` with proper query-param parsing (`split('&')`) + constant-time compare
  - Remove token from `log::info!` URLs; print once via `eprintln!` at startup
- **WS hardening** (Fixes #9):
  - Add `active_ws_count: AtomicUsize` to `WebServerState`, cap at `MAX_WS_CONNECTIONS = 32`
  - Add per-connection rate limit (max 60 msg/sec) and max message size (64 KB text frame)
- **WS Origin check** (Fixes #10):
  - Verify `Origin` header host matches request `Host` when `!lan_trust`; allow any non-empty origin when `lan_trust`
- **CORS**: Remove `CorsLayer::permissive()` — the consolidated router uses no CORS (same-origin default), which is correct for a token-auth control surface

### `rustjay-api` (guest)
- **Delete `server.rs`** entirely (no `ApiConfig`, `ApiServerHandle`, `start()`, no thread, no runtime)
- **Delete `ws.rs`** entirely (Fixes #14 — reuse control's existing broadcast/diff WS at `/{app}/ws`)
- **Delete `ApiState` + `publish()`** (Fixes #11 — no per-frame deep clone)
- Redefine `SharedState`:
  ```rust
  pub struct SharedState {
      pub command_tx: mpsc::Sender<rustjay_control::WebCommand>,
      pub engine_state: Arc<std::sync::Mutex<rustjay_core::EngineState>>,
  }
  ```
- `build_router(shared)` returns a `Router` containing:
  - All typed REST routes (`/api/health`, `/api/state`, `/api/params`, `/api/input/...`, etc.)
  - `SwaggerUi::new("/swagger-ui").url("/api/openapi.json", ApiDoc::openapi())` — merged *inside* the protected router, so Swagger requires the token (Fixes #8)
  - `DefaultBodyLimit::max(1024 * 1024)`
  - **No** CORS layer, **no** auth middleware, **no** runtime
- Handlers read state on-demand: lock `EngineState`, build DTO, drop lock, return `Json(dto)` (no pre-built snapshot)
- Replace all `Json(serde_json::to_value(x).unwrap())` with direct `Json(x)` (Fixes #17 — removes infallible unwraps)
- Remove dead `web_start` / `web_stop` handlers (they send fake `Set` commands the engine never handles)
- Update `tests.rs` — auth now applies to `/api/health` because the router is always behind control's auth layer

### `rustjay-engine` (composition root)
- `app/mod.rs`:
  - Remove `ApiState::new`, `ApiConfig`, `start_server` (lines 334-351)
  - Remove `api_state` and `api_handle` fields
  - When `feature = "api"`, create `rustjay_api::SharedState` and call `web_server.set_api_router(...)` before `web_server.start()`
- `app/update.rs`:
  - Remove `api_state.publish(&state)` block (lines 515-520)
- `gles2.rs`:
  - Mirror the same API router wiring in the DRM/GLES2 init path (consistent with desktop)

---

## 4. Issue Resolution Mapping

| Issue | Resolution |
|-------|------------|
| #7 auto-start on 0.0.0.0 | API inherits `WebConfig::enabled = false` by default; no auto-start. LAN bind remains default per 1.4-R, but token is required. |
| #8 Swagger bypasses auth | Swagger UI is now merged **inside** the protected router that has `route_layer(auth_middleware)` |
| #9 WS unbounded channel | API's custom WS deleted; control's WS gets max-connections cap + rate limit + size limit |
| #10 permissive CORS + no WS Origin | CORS removed; WS Origin check added (matches Host when strict, any when lan_trust) |
| #11 per-frame deep clone | `ApiState::publish()` deleted; REST reads build snapshot on-demand inside the handler lock |
| #12 second tokio runtime | `server.rs` deleted; API spawns **zero** threads/runtimes |
| #13 second server on port+1 | `port+1` binding deleted; one listener only |
| #14 WS re-serialize per client | API's `/api/ws` deleted; reuse control's single-serialization broadcast path |
| #15 REST is subset of WS vocab | All handlers map to the **full** `rustjay_control::WebCommand` enum and sub-enums (`InputWebCommand`, `OutputWebCommand`, `AudioWebCommand`, `LinkWebCommand`, `ProDjWebCommand`, etc.) |
| #16 auth hardening | Fixed in `rustjay-control::auth_middleware`: constant-time compare, parsed query param, token removed from logs |
| #17 dead handlers / unwraps | Remove `web_start`/`web_stop`; replace `serde_json::to_value(...).unwrap()` with direct `Json(...)`; fix stale test comments |
| DOM-XSS (1.3) | N/A for JSON API. Swagger UI serves static embedded assets from `utoipa-swagger-ui` (trusted). No user-controlled HTML rendering. |

---

## 5. Dependency & Feature Invariants

- `rustjay-core` gains **zero** new dependencies.
- `rustjay-control` does **not** depend on `rustjay-api`.
- `rustjay-api` remains `optional = true` in `rustjay-engine/Cargo.toml`, off by default.
- `cargo build -p delta` (no `api` feature) is byte-for-byte unaffected.
- Pi/GLES2 path (`gles2.rs`) wires the API router identically.

---

## 6. Verification Plan

| Check | How |
|-------|-----|
| `cargo check --workspace` | CI gate |
| `cargo check -p rustjay-engine --features api` | API compiles |
| `cargo check -p delta` | Unchanged apps unaffected |
| `cargo clippy -p rustjay-api -- -D warnings` | Clean |
| `cargo test -p rustjay-api` | Updated tests pass (auth now on `/api/health`) |
| `cargo test -p rustjay-control` | WS rate-limit / connection-cap tests added |
| Live: one listener | `lsof -i :8081` only; nothing on `:8082` |
| Live: auth on Swagger | `curl /swagger-ui` → 401; with `Authorization: Bearer <token>` → 200 |
| Live: REST round-trip | `PUT /api/params` → engine reflects value in next WS broadcast |
| Live: WS rejects bad Origin | `websocat` with wrong `Origin` → 403 |
| Live: LAN default | Default host `0.0.0.0`, token auto-generated; `--bind 127.0.0.1` opt-in hardening |

---

**Sign-off requested before implementation.**
