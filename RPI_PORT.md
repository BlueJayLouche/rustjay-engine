# Raspberry Pi Port — Web Remote Appliance

Extend the web remote from a parameter-slideshow into a full control surface that replaces the desktop GUI for headless Pi operation. Three new panels — Input, Control, Modulation — each open in their own window so the main slider view stays uncluttered.

> **Target**: Pi 2 / Pi 3 / Pi 4 running `--nogui --gles2 --drm` with LAN-trust web access as the sole control surface.

---

## Architectural Principles

1. **Single source of truth.** `EngineState` remains the authoritative state. The web remote reflects it; it does not shadow it.
2. **Command pattern.** Web clients send commands (intent), not state (values). The engine validates and applies them on the next frame — identical to OSC/MIDI.
3. **Panel isolation.** Each major feature lives in its own HTML panel that opens via a button on the main page. Panels share the same WebSocket but subscribe to different message scopes.
4. **Embedded-first.** Every feature must work on the GLES2/DRM path where the renderer owns its own capture thread. Desktop/wgpu path must not regress.

---

## Architecture Notes (from review)

### The two `WebCommand` types

There are two distinct `WebCommand` types in the codebase. **They must not be confused.**

| Type | Crate | Location | Purpose |
|---|---|---|---|
| `rustjay_core::WebCommand` | `rustjay-core` | `EngineState::web_command` field | **Lifecycle only**: `Start`, `Stop`, `SetPort`, `SetLanTrust` — driven by the egui GUI |
| `rustjay_control::web::WebCommand` | `rustjay-control` | drained from `web_server.command_rx` | **Wire protocol**: commands arriving from HTTP clients; currently only `Set { id, value }` |

All new `Input / Control / Modulation` variants go on the **wire** type (`rustjay_control::web::WebCommand`). The lifecycle enum (`rustjay_core::WebCommand`) is untouched.

The desktop engine processes the wire commands in `process_web_commands()` inside `crates/rustjay-engine/src/app/commands.rs` (the `while let Ok(cmd) = server.command_rx.try_recv()` block at the bottom of that function). The GLES2/DRM loop processes them in the equivalent block at `crates/rustjay-engine/src/gles2.rs:528`.

### Command routing on the GLES2/DRM path

All structural commands (`Input*`, `Control*`, `Modulation*`) are dispatched in the `run_drm_gles2_loop` main loop (not through `App<P>::dispatch_commands`, which only runs on the desktop path). The GLES2 loop dispatches before the render call so input-device teardown / setup never overlaps with a frame in flight.

### Panels use HTTP POST for writes, WebSocket for reads

`App.sendCmd(cmd)` always goes through `POST /{app_name}/cmd`. The WebSocket is read-only from the client's perspective (it receives broadcast state). This eliminates race conditions between concurrent WS message handlers. The POST handler is inside the `protected` router so auth middleware applies automatically.

---

## Implementation Progress

> Last updated: 2026-05-31

### ✅ Completed

| Step | Description | Files Modified |
|---|---|---|
| WR-1.1 | Extended `rustjay_control::web::WebCommand` with `Input`, `Control`, `Modulation`, `Preset` variants; fixed serde to `#[serde(tag="action")]` on sub-enums | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.2 | Added `POST /{app}/cmd` route inside protected router | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.3 | Added `WebMessage` variants and JSON structs; extended `ControlStateJson` with full MIDI state; added `bpm`/`tap_tempo_info` to `ModulationStateJson` | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.4 | Added dirty flags to `WebServer` | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.5 | Broadcast methods; `send_preset_state` updates `preset_names`; WS connect sends all cached states | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.6 | `preset_names`, `pending_devices`, `last_refresh`, `last_*_state` caches in `WebServerState` | `crates/rustjay-control/src/web/mod.rs` |
| WR-1.7 | `InputDeviceInfo` in `rustjay-core`; `available_devices` in `InputState` | `crates/rustjay-core/src/state.rs` |
| WR-1.8 | `Lfo::last_beat_phase` public | `crates/rustjay-core/src/lfo.rs` |
| WR-1.9 | Re-exported JSON structs from `rustjay-control` lib | `crates/rustjay-control/src/lib.rs` |
| WR-2.1 | Async `v4l2-ctl --list-devices` in `cmd_handler` with 5-second throttle; GLES2 loop polls `pending_devices` each frame | `crates/rustjay-control/src/web/mod.rs`, `crates/rustjay-engine/src/gles2.rs` |
| WR-2.2 | `Input` command dispatch in GLES2 loop (no `shared_state` lock during webcam open) | `crates/rustjay-engine/src/gles2.rs` |
| WR-2.3 | Input panel HTML (`input.html`) — device list, PAL/NTSC dropdown, stop/restart | `crates/rustjay-control/src/web/input.html` |
| WR-2.4 | `get_input_state()` on `Gles2Effect`/`Gles2EffectDyn`; `FluxGles2` tracks active device index/resolution | `crates/rustjay-engine/src/gles2.rs`, `examples/flux/src/gles2_renderer.rs` |
| WR-3.1 | MIDI learn with `param_descriptors` lookup; `MidiRefreshDevices`, `MidiSelectDevice`, `MidiDisconnect` in both loops | `crates/rustjay-engine/src/app/commands.rs`, `crates/rustjay-engine/src/gles2.rs` |
| WR-3.2 | Control panel HTML (`control.html`) — OSC, MIDI device list, parameter learn buttons, mapping list, learn badge | `crates/rustjay-control/src/web/control.html` |
| WR-3.3 | MIDI mapping change detection per-frame in GLES2; writes to `state.midi_mappings`; triggers `save_settings_requested` | `crates/rustjay-engine/src/gles2.rs` |
| WR-3.4 | MIDI device + mappings persist across restarts via `AppSettings`; GLES2 init reconnects saved device | `crates/rustjay-engine/src/config.rs`, `crates/rustjay-engine/src/gles2.rs` |
| WR-4.1 | `param_id_to_modulation_target` helper | `crates/rustjay-engine/src/gles2.rs` |
| WR-4.2 | GLES2 dispatch for `Control*` (Osc, OscSetPort, MidiLearn, MidiLearnCancel, MidiUnlearn, MidiRefreshDevices, MidiSelectDevice, MidiDisconnect) | `crates/rustjay-engine/src/gles2.rs` |
| WR-4.3 | GLES2 dispatch for `Modulation*` (LfoSet with phase continuity, LfoEnable, TapTempo) | `crates/rustjay-engine/src/gles2.rs` |
| WR-4.4 | GLES2 dispatch for `Preset*` (List, Save with name validation, Load, Delete) | `crates/rustjay-engine/src/gles2.rs` |
| WR-4 HTML | Modulation panel HTML (`modulation.html`) — LFO slots with custom param targets, tempo sync, rate, tap tempo, audio routes display | `crates/rustjay-control/src/web/modulation.html` |
| WR-5 | Desktop dispatch for all new variants in `commands.rs` + dirty-flag drain in `update.rs` | `crates/rustjay-engine/src/app/commands.rs`, `crates/rustjay-engine/src/app/update.rs` |
| WR-5.1 | Panel routes (`/input`, `/control`, `/modulation`, `/presets`) inside protected router | `crates/rustjay-control/src/web/mod.rs` |
| WR-5.2 | Toolbar `[Input][Control][Modulation][Presets]` buttons with token forwarding | `crates/rustjay-control/src/web/ui.html` |
| WR-6 | Dirty-flag drain in both loops; MIDI mapping changes trigger save immediately | `crates/rustjay-engine/src/gles2.rs`, `crates/rustjay-engine/src/app/update.rs` |
| WR-8 | Auto-detect read-only root at GLES2 startup; redirect writes to `/boot/rustjay-data` in-process if needed; service file documents permanent fix | `crates/rustjay-engine/src/gles2.rs`, `guide/src/deployment/raspberry-pi.md` |
| WR-9.1 | Device enumeration async bridge (shared with WR-2.1) | `crates/rustjay-control/src/web/mod.rs` |
| WR-9.3 | `PresetBank` kept alive for GLES2/DRM loop duration | `crates/rustjay-engine/src/gles2.rs` |
| WR-9.4 | Presets panel HTML (`presets.html`) — save-as, load, delete with confirm, 2-second write-failure toast | `crates/rustjay-control/src/web/presets.html` |
| WR-9.5 | Panel routes and toolbar buttons (see WR-5.1/5.2) | |
| WR-9.6 | Initial `PresetState` broadcast on WebSocket connect | `crates/rustjay-control/src/web/mod.rs` |

### Bug fixes applied during runtime testing

| Fix | Description |
|---|---|
| Serde untagged ambiguity | `Load { index }` vs `Delete { index }` under `#[serde(untagged)]` — `Delete` was unreachable. Fixed via `#[serde(tag="action")]` on all four sub-enums. |
| LFO custom target ID mismatch | `modulation.html` stored the full web path `"category/id"` in `LfoTarget::Custom`; backend matched bare `"id"`. LFO had zero effect on custom parameters. Fixed by stripping category prefix before storage. |
| Tap tempo 4-tap gate | BPM only computed after ≥ 4 taps; first 3 taps were silent. Reduced to 2 (one interval). |
| Tap tempo no broadcast | `modulation_dirty` never set after tap; BPM display never updated. Fixed. |
| Dirty flags never drained | `send_control_state`/`send_preset_state` not called after flags set — panels never received state after initial connect. Fixed. |
| MIDI/OSC fall-through | `MidiLearn`, `MidiLearnCancel`, `MidiUnlearn` fell through `_ => {}` in GLES2 Control arm. Fixed. |
| `log::warn!` as trace | Three per-frame/per-connect `log::warn!` lines demoted to `log::debug!`/`info!`/removed. |

### ⏳ Remaining

| Step | Description |
|---|---|
| WR-7 | Full compile matrix verification and Pi hardware runtime testing |
| AudioRoute / AudioUnroute | `AudioRoutingState` mutation API unverified; modulation panel shows routes read-only for now |

---

## Task breakdown

---

### WR-1 — Web command protocol expansion

**Crates**: `rustjay-control`, `rustjay-core`, `rustjay-engine`

The current web command channel carries only `Set { id, value }`. Expand it to carry structural commands.

#### 1.1 Extend `rustjay_control::web::WebCommand` (the wire type)

> **Note:** This is `rustjay_control::web::WebCommand` — the type drained from `web_server.command_rx`. It is **not** `rustjay_core::WebCommand` (the lifecycle enum). Do not modify the core lifecycle enum.

```rust
// crates/rustjay-control/src/web/mod.rs
pub enum WebCommand {
    /// Parameter value change (existing)
    Set { id: String, value: f32 },

    /// Input subsystem
    Input(InputWebCommand),

    /// MIDI / OSC subsystem
    Control(ControlWebCommand),

    /// LFO / audio-routing subsystem
    Modulation(ModulationWebCommand),
}

pub enum InputWebCommand {
    /// Refresh the device list and broadcast it to all clients
    RefreshDevices,
    /// Switch to a specific capture device
    SelectDevice { index: usize, width: u32, height: u32, fps: u32 },
    /// Stop the current input
    StopInput,
}

pub enum ControlWebCommand {
    /// Start / stop OSC server
    Osc { enabled: bool },
    /// Change OSC listen port
    OscSetPort { port: u16 },
    /// Enter MIDI learn mode for a specific parameter.
    /// The handler MUST look up `name`, `min`, `max` from `EngineState::param_descriptors`
    /// before calling `MidiCommand::StartLearn` — the web wire only carries `param_id`.
    MidiLearn { param_id: String },
    /// Cancel MIDI learn without mapping anything
    MidiLearnCancel,
    /// Remove a MIDI mapping
    MidiUnlearn { cc: u8, channel: u8 },
}

pub enum ModulationWebCommand {
    /// Create or update an LFO slot.
    /// Use `rustjay_core::lfo::Lfo` directly as the config type — it already derives
    /// Serialize/Deserialize and all runtime-only fields are #[serde(skip)].
    LfoSet { slot: usize, config: rustjay_core::lfo::Lfo },
    /// Enable / disable an LFO
    LfoEnable { slot: usize, enabled: bool },
    /// Route an audio FFT band to a parameter
    AudioRoute { param_id: String, band: rustjay_core::FftBand, depth: f32 },
    /// Clear all audio routes for a parameter
    AudioUnroute { param_id: String },
}
```

#### 1.2 Add `POST /{app_name}/cmd` route inside `create_router`

Add the route **inside** the `protected` `Router` in `crates/rustjay-control/src/web/mod.rs` so the auth middleware covers it automatically:

```rust
// Inside create_router(), add to `protected`:
.route(&cmd_path, post(cmd_handler))
```

The handler accepts JSON-encoded `WebCommand`, validates it, pushes into `command_tx`, returns `200 OK` on enqueue, `503` if the channel is full.

> **Auth:** Because the route is inside `protected`, `auth_middleware` (bearer token or `lan_trust`) applies with no extra code. Do not add a separate auth check inside `cmd_handler`.

#### 1.3 Add WebSocket broadcast scopes

The existing WebSocket broadcasts `WebMessage::Params` and `WebMessage::Update` to all clients. Add scoped broadcasts so panels only receive relevant state:

```rust
// New message types pushed from engine → web clients
// Add to the existing WebMessage enum
enum WebMessage {
    // existing variants …
    InputState(InputStateJson),       // device list + active device
    ControlState(ControlStateJson),   // OSC on/off, port, MIDI mappings
    ModulationState(ModulationStateJson), // LFOs, audio routes
}
```

Define `InputStateJson`, `ControlStateJson`, `ModulationStateJson` as simple `serde`-derived structs in the same file. Add a `send_input_state()`, `send_control_state()`, and `send_modulation_state()` method on `WebServer` that serialise and push the relevant `WebMessage` variant.

#### 1.4 Handle new commands in the desktop app loop

`crates/rustjay-engine/src/app/commands.rs` — extend the `while let Ok(cmd) = server.command_rx.try_recv()` block inside `process_web_commands()`:

- `Input(InputWebCommand::SelectDevice { index, width, height, fps })` → write `InputCommand::StartWebcam { device_index: index, width, height, fps }` into `state.input_command`.
- `Input(InputWebCommand::StopInput)` → write `InputCommand::StopInput`.
- `Input(InputWebCommand::RefreshDevices)` → write `InputCommand::RefreshDevices`.
- `Control(ControlWebCommand::Osc { enabled })` → write `OscCommand::Start` / `OscCommand::Stop`.
- `Control(ControlWebCommand::OscSetPort { port })` → write `OscCommand::SetPort(port)`.
- `Control(ControlWebCommand::MidiLearn { param_id })` → look up `param_id` in `state.param_descriptors`, extract `name`, `min`, `max`, then write `MidiCommand::StartLearn { param_path: param_id, param_name: name, min, max }`. Log a warning and skip if the param is not found.
- `Control(ControlWebCommand::MidiLearnCancel)` → write `MidiCommand::CancelLearn`.
- `Control(ControlWebCommand::MidiUnlearn { cc, channel })` → find the matching mapping in `MidiState` and remove it directly (no existing `MidiCommand` variant for this; add one or mutate `MidiState` directly via `midi_manager.state().lock()`).
- `Modulation(ModulationWebCommand::LfoSet { slot, config })` → bounds-check `slot`, then `state.lfo.bank.lfos[slot] = config` (preserve `phase`/`output` if you want continuity — copy them from the existing slot before overwriting).
- `Modulation(ModulationWebCommand::LfoEnable { slot, enabled })` → `state.lfo.bank.lfos[slot].enabled = enabled`.
- `Modulation(ModulationWebCommand::AudioRoute { param_id, band, depth })` → call the appropriate `AudioRoutingState` mutation method.
- `Modulation(ModulationWebCommand::AudioUnroute { param_id })` → remove all routes for `param_id`.

#### 1.5 Handle new commands in the GLES2/DRM loop

`crates/rustjay-engine/src/gles2.rs` — extend the `while let Ok(cmd) = web_server.command_rx.try_recv()` block (currently at line ~528).

The `Input*` variants are **different** from the desktop path: the GLES2 renderer owns its own webcam inside `FluxGles2`, not through `InputManager`. See the `Gles2Effect` trait extension below (Finding F2).

For `Control*` and `Modulation*` variants the handling is identical to the desktop path: write into `shared_state`.

> **Do not** write `InputCommand` variants into `shared_state.input_command` on the GLES2 path — `InputManager` is not instantiated in that path. Input commands must go through the `Gles2Effect::handle_input_command` trait method described in WR-2.

#### 1.6 Extend the `Gles2Effect` / `Gles2EffectDyn` trait for input command dispatch

> **This is a blocking prerequisite for WR-2.** Without this, there is no type-safe way to call `FluxGles2::open_webcam` from the `run_drm_gles2_loop` which holds a `Box<dyn Gles2EffectDyn>`.

In `crates/rustjay-engine/src/gles2.rs`, add to both traits:

```rust
// Public trait
pub trait Gles2Effect: Send + 'static {
    // … existing methods …

    /// Called from the run loop when an InputWebCommand arrives.
    /// Default implementation is a no-op so existing effects don't need to change.
    fn handle_input_command(&mut self, _gl: &glow::Context, _cmd: InputWebCommand) {}
}

// Type-erased wrapper — add the matching forwarding method
pub(crate) trait Gles2EffectDyn: Send + 'static {
    // … existing methods …
    fn handle_input_command(&mut self, gl: &glow::Context, cmd: InputWebCommand);
}
impl<G: Gles2Effect> Gles2EffectDyn for G {
    fn handle_input_command(&mut self, gl: &glow::Context, cmd: InputWebCommand) {
        Gles2Effect::handle_input_command(self, gl, cmd);
    }
}
```

Then in `run_drm_gles2_loop`, when an `Input(cmd)` variant is received, call:

```rust
let gl = gles2_state.gl.clone();
gles2.handle_input_command(&gl, cmd);
```

---

### WR-2 — Input Panel

**Crates**: `rustjay-control` (web server + HTML), `rustjay-engine` (command handling + trait)

A standalone panel showing available V4L2/webcam devices, the active input, and its negotiated resolution.

#### 2.1 Device enumeration — shared state bridge

Device enumeration spans two threads (web server Tokio runtime; GLES2 main loop). A new shared primitive bridges them:

```rust
// Add to WebServerState (or pass separately into run_drm_gles2_loop):
pub pending_devices: Arc<Mutex<Option<Vec<InputDeviceInfo>>>>,
```

Flow:

1. `RefreshDevices` command arrives via `cmd_handler` on the Tokio thread.
2. `cmd_handler` spawns `tokio::process::Command::new("v4l2-ctl").args(["--list-devices"])`, awaits output, parses it into `Vec<InputDeviceInfo>`, writes result into `pending_devices`.
3. On the next main-loop iteration, `run_drm_gles2_loop` checks `pending_devices`. If `Some`, moves the list into `shared_state.input.available_devices` (add this field to `InputState`) and clears `pending_devices`. The next web broadcast picks it up.

> **Why not run enumeration on the main thread?** `v4l2-ctl` can block for ~200ms on a Pi 2 with a slow capture device. Blocking the render loop for that long would cause a visible frame drop. Keeping it async on the Tokio thread avoids this.

Throttle: track `last_refresh: Instant` inside the handler and return `429` if called within 5 seconds of the previous refresh.

**Desktop path**: `InputManager::v4l2_capture_devices()` already exists. Expose it through `EngineState::input.available_devices` via the existing `InputCommand::RefreshDevices` path (which calls `manager.begin_refresh_devices()`).

#### 2.2 `FluxGles2::handle_input_command` implementation

Implement `Gles2Effect::handle_input_command` on `FluxGles2` (in `examples/flux/src/gles2_renderer.rs`):

```rust
fn handle_input_command(&mut self, _gl: &glow::Context, cmd: InputWebCommand) {
    match cmd {
        InputWebCommand::SelectDevice { index, width: _, height: _, fps: _ } => {
            // Stop existing capture, open new device.
            // Use the video_standard from EngineState if available; otherwise default PAL.
            self.receiver  = None;
            self.webcam    = None;
            self.last_frame = None;
            self.open_webcam(index, 0 /* PAL default */);
        }
        InputWebCommand::StopInput => {
            self.receiver   = None;
            self.webcam     = None;
            self.last_frame = None;
        }
        InputWebCommand::RefreshDevices => {
            // Enumeration is handled by the web server async path; nothing to do here.
        }
    }
}
```

> **Video standard:** `SelectDevice` in the web command carries `width/height/fps`. For the GLES2 path these are currently ignored because `open_webcam` derives resolution from the video standard (PAL 720×576@25, NTSC 720×480@30). The handler should read `video_standard` from the current `EngineState` if it wants to honour the requested resolution. For the initial implementation, defaulting to PAL is acceptable — just document the limitation.

> **Latency SLA (2 seconds):** `WebcamCapture::new()` involves a `V4L2_CAP_*` ioctl that can block on Pi 2. `open_webcam` should not be called while holding any `shared_state` lock. The trait method receives `&mut self` (the effect), not the state lock — so this is safe as written above. Verify no state lock is held in the call site in `run_drm_gles2_loop`.

#### 2.3 HTML panel — Input Window

New embedded HTML/JS in `crates/rustjay-control/src/web/` as `input.html` (loaded via `include_str!`):

```
┌─ Input --------------------------------┐
│ [Refresh]                              │
│ ◉ AV TO USB2.0   /dev/video0  720×576  │
│ ○ None (offline)                       │
│                                        │
│ Resolution: [720×576 ▼]  FPS: [25 ▼]   │
│ Standard:   [PAL   ▼]                  │
│ [Restart Input]                        │
└────────────────────────────────────────┘
```

- **Device list:** radio buttons. Selecting one sends `Input(SelectDevice { index, width, height, fps })` via `POST /cmd`.
- **Resolution dropdown:** populated from the device's format list. Re-opens the device on change.
- **Standard dropdown:** PAL / NTSC. Maps to the `video_standard` parameter (already implemented in `FluxState`).
- **Restart button:** sends `Input(StopInput)` followed by `Input(SelectDevice)` with current settings.

#### 2.4 State broadcast

After each `handle_input_command` call in the GLES2 loop, set a `input_dirty = true` flag. At the end of the frame's web broadcast section, if `input_dirty`, call `web_server.send_input_state(...)` and clear the flag.

---

### WR-3 — Control Panel (MIDI + OSC)

**Crates**: `rustjay-control`, `rustjay-engine`

A panel for MIDI mapping management and OSC server configuration.

#### 3.1 OSC control

The engine already supports `OscCommand::Start` / `OscCommand::Stop` / `OscCommand::SetPort`.

- Expose current OSC state (`enabled`, `host`, `port`) via `BroadcastMsg::ControlState`.
- Add toggle switch `[ ] OSC Enabled` and port number field.
- On change, send `Control(Osc { enabled })` or `Control(OscSetPort { port })`.

#### 3.2 MIDI learn — full flow with parameter translation

**Desktop path:** `MidiManager` already handles `MidiCommand::Learn` / `MidiCommand::Unlearn`.

**Embedded path:** Same `MidiManager` is instantiated in `gles2.rs`. No extra work.

> **Translation (F5):** `ControlWebCommand::MidiLearn { param_id }` carries only a string id. The dispatch handler (WR-1.4 / 1.5) must look up the parameter in `EngineState::param_descriptors` to obtain `name`, `min`, and `max` before calling `MidiCommand::StartLearn`. If the param is not found, log a warning and skip — do not call `StartLearn` with zero bounds.

**HTML panel:**

```
┌─ Control ------------------------------┐
│ OSC: [✓] Enabled   Port: [9001]        │
│                                        │
│ MIDI Mappings                          │
│ CC  14 Ch 1 → flow_scale               │
│ CC  15 Ch 1 → warp_strength   [✕]      │
│                                        │
│ [Learn]  Select a parameter, then      │
│          move a MIDI CC to map it.     │
└────────────────────────────────────────┘
```

**Learn flow:**
1. User clicks `[Learn]` next to a parameter.
2. Web client sends `Control(MidiLearn { param_id: "flow_scale" })`.
3. Engine enters learn mode: the next MIDI CC received on any channel maps to that parameter.
4. Engine sends updated `ControlState` — see 3.3.
5. Web UI highlights the newly mapped row.

**Unlearn:** Click `[✕]` sends `Control(MidiUnlearn { cc, channel })`.

#### 3.3 MIDI mapping change detection and broadcast (F4)

`MidiLearn` completes asynchronously inside the `MidiManager` background thread. The web client will not see the new mapping unless the main loop actively detects the change and pushes a broadcast.

Add to the per-frame section of both the desktop loop (`process_web_commands`) and the GLES2 loop:

```rust
// Track last-broadcast snapshot across frames (add to loop state, not EngineState):
let mut last_broadcast_mappings: Vec<MidiMappingSnapshot> = Vec::new();

// Each frame, after draining commands:
if let Some(ref manager) = midi_manager {
    if let Ok(midi_st) = manager.state().lock() {
        let current: Vec<MidiMappingSnapshot> = midi_st.mappings.iter()
            .map(MidiMappingSnapshot::from)
            .collect();
        if current != last_broadcast_mappings {
            last_broadcast_mappings = current.clone();
            web_server.send_control_state(ControlStateJson {
                osc_enabled: …,
                osc_port: …,
                midi_mappings: current,
            });
        }
    }
}
```

This O(n) comparison runs only when MIDI state exists and is a low-frequency path (mappings change rarely during performance).

#### 3.4 MIDI mapping persistence

Mappings are already persisted in `~/.config/rustjay/<app>.json` via `AppSettings`. No new persistence layer needed.

---

### WR-4 — Modulation Panel (LFO + Audio Routing)

**Crates**: `rustjay-core`, `rustjay-control`, `rustjay-engine`

A panel for creating LFOs and mapping audio FFT bands to parameters.

#### 4.1 LFO configuration

`rustjay-core` already has `LfoState` with 8 slots (field is `lfo.bank.lfos`, not `lfo.slots`), each carrying waveform, rate, depth, and target.

> **`LfoConfigJson` does not exist and should not be created.** `rustjay_core::lfo::Lfo` already derives `Serialize/Deserialize`. All runtime-only fields (`phase`, `output`, `last_beat_phase`) are `#[serde(skip)]`, so deserialising a `Lfo` from JSON yields only configuration fields. Use `Lfo` directly as the command payload.

> **Preserving LFO phase continuity on update:** When processing `LfoSet`, copy `phase`, `output`, and `last_beat_phase` from the existing slot into the incoming `config` before writing it, so an in-flight LFO doesn't snap to phase 0 when its config is tweaked:
> ```rust
> let existing = &state.lfo.bank.lfos[slot];
> let mut config = config;
> config.phase         = existing.phase;
> config.output        = existing.output;
> config.last_beat_phase = existing.last_beat_phase;
> state.lfo.bank.lfos[slot] = config;
> ```

**HTML panel:**

```
┌─ Modulation ----------------------------┐
│ LFO 1: [✓] Sine  1/4 beat  → hue_shift │
│        Depth: [0.50]                    │
│                                         │
│ LFO 2: [ ] Off                         │
│        [+ Add LFO]                     │
│                                         │
│ Audio Reactivity                        │
│ Bass    → flow_scale      Depth [0.30]  │
│ Mid     → —               Depth [0.00]  │
│ Treble  → feedback_decay  Depth [0.20]  │
└─────────────────────────────────────────┘
```

- **LFO slot:** dropdown for waveform (Sine, Triangle, Square, Ramp, Saw — matching `Waveform` enum), beat division, target parameter, depth slider.
- **Audio routing:** 8-band FFT → parameter matrix. Each row is a band + parameter dropdown + depth slider.

#### 4.2 Command handling

`ModulationWebCommand::LfoSet` carries a `Lfo` (not `LfoConfigJson`). The engine deserialises it, copies phase continuity fields, and writes it to `state.lfo.bank.lfos[slot]`.

`ModulationWebCommand::AudioRoute` updates `AudioRoutingState` via its existing mutation API. Verify what methods are available on `AudioRoutingState` before implementing — do not guess the API.

`ModulationWebCommand::LfoEnable` is a shorthand that sets `state.lfo.bank.lfos[slot].enabled` without touching other fields.

#### 4.3 State broadcast

Push `BroadcastMsg::ModulationState` whenever LFO or audio routing changes. The web client diffs against its local state to avoid full re-renders.

---

### WR-5 — Panel UI framework

**Crate**: `rustjay-control` (web HTML/JS)

The existing web UI is a single monolithic HTML string. Refactor it into a small SPA with panel navigation.

#### 5.1 Route structure

- `GET /{app_name}` — Main page with parameter sliders (existing)
- `GET /{app_name}/input` — Input panel (new)
- `GET /{app_name}/control` — MIDI + OSC panel (new)
- `GET /{app_name}/modulation` — LFO + audio panel (new)
- `POST /{app_name}/cmd` — Command endpoint (new, WR-1.2)

All panels share the same WebSocket connection at `/{app_name}/ws`.

Each panel route serves its own `include_str!`'d HTML file. Each file gets the same `inject_token_into_html` treatment as the main page so `window.RUSTJAY_TOKEN` is available client-side.

> All four routes must be added inside `create_router`'s `protected` `Router` so the auth middleware covers them.

#### 5.2 JS architecture (vanilla, no build step)

```javascript
// Main app shell — loaded on every panel
const App = {
  ws: null,
  state: {},          // last-known EngineState subset
  panels: {},         // panel renderers

  init() {
    this.ws = new WebSocket(`ws://${location.host}/${APP_NAME}/ws?token=${window.RUSTJAY_TOKEN}`);
    this.ws.onmessage = (e) => this.handleMessage(JSON.parse(e.data));
  },

  handleMessage(msg) {
    switch (msg.type) {
      case 'params':      this.panels.main?.update(msg); break;
      case 'update':      this.panels.main?.update(msg); break;
      case 'InputState':  this.panels.input?.update(msg); break;
      case 'ControlState':    this.panels.control?.update(msg); break;
      case 'ModulationState': this.panels.modulation?.update(msg); break;
    }
  },

  sendCmd(cmd) {
    fetch(`/${APP_NAME}/cmd?token=${window.RUSTJAY_TOKEN}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(cmd)
    });
  }
};
```

Each panel is a self-contained script that registers itself with `App.panels` if its DOM container exists.

#### 5.3 Panel launcher on main page — auth and mobile scope

Add a floating toolbar to the main slider view:

```
[Input] [Control] [Modulation]
```

> **Auth token must be forwarded.** Panel windows are opened as:
> ```js
> window.open(`/${APP_NAME}/input?token=${window.RUSTJAY_TOKEN}`, '_blank')
> ```
> Without the `?token=` query param, the auth middleware returns `401 Unauthorized`. Each `window.open` call must include the token. The panel HTML itself also needs `inject_token_into_html` applied so `window.RUSTJAY_TOKEN` is available for its own WebSocket and `sendCmd` calls.

> **Scope: desktop browsers only.** Panel windows use `window.open(…, '_blank')`. On iOS Safari and most Android browsers this is blocked by default popup blocking unless triggered by a direct user gesture on a trusted origin. This is accepted behaviour — the target is a desktop browser or phone browser where popups are permitted. Do **not** attempt a mobile-native single-page fallback at this stage. Add a comment in the HTML: `<!-- panels open in new tabs; requires popup permission on mobile browsers -->`.

---

### WR-6 — State sync efficiency

**Crate**: `rustjay-control`

The current `update_parameter()` is called every frame for every parameter. With three new panels, broadcast traffic could explode for the structural channels.

#### 6.1 Dirty-flag broadcasting for structural channels

> **Note on the param channel:** The existing `last_sent: HashMap<String, f32>` with a `0.001` threshold already suppresses most redundant parameter broadcasts. The dirty-flag system described here is primarily valuable for the new `Input / Control / Modulation` state channels, where no equivalent throttle exists yet. Do not re-implement the param channel unless profiling shows the `last_sent` cache is insufficient.

Add dirty flags to `WebServer` (not `WebServerState`):

```rust
// In WebServer (not in WebServerState — these are per-flush flags, not shared state)
pub struct WebServer {
    // … existing fields …
    pub input_dirty:      bool,
    pub control_dirty:    bool,
    pub modulation_dirty: bool,
}
```

- Set `input_dirty = true` after any `handle_input_command` call.
- Set `control_dirty = true` after any MIDI mapping change is detected (WR-3.3).
- Set `modulation_dirty = true` after any `LfoSet` / `AudioRoute` command.
- At the end of each frame's web broadcast section, drain dirty flags and send the appropriate `send_*_state()` calls.

#### 6.2 Throttle non-param broadcasts

`send_input_state`, `send_control_state`, `send_modulation_state` are only called when the corresponding dirty flag is set. This guarantees no busy-wait broadcast on quiet frames.

---

### WR-7 — Integration & verification

#### 7.1 Compile matrix

| Target | Features | Must compile |
|--------|----------|--------------|
| macOS desktop | default | ✅ |
| Pi 4 | `--no-default-features --features webcam,drm-gles2,midi` | ✅ |
| Pi 3 | `--no-default-features --features webcam,drm-gles2,midi` | ✅ |
| Pi 2 | `--no-default-features --features webcam,drm-gles2,midi` | ✅ |

> **Pi 3 added.** Uses the same `drm-gles2` path as Pi 4. Verify `midi` is not excluded by `no-default-features` before including it in the feature set — check `Cargo.toml`.

#### 7.2 Runtime checklist

- [ ] Open Input panel → token in URL, no `401`
- [ ] Open Input panel → see AV TO USB2.0 listed
- [ ] Select device → video appears within 2 seconds
- [ ] Change resolution → texture recreates, render continues
- [ ] Open Control panel → see current OSC port
- [ ] Toggle OSC off → `ss -lnp` no longer shows port 9000
- [ ] Click MIDI Learn → move CC knob → mapping appears in panel (tests WR-3.3 broadcast)
- [ ] Unlearn mapping → `[✕]` removes it, broadcast pushes update
- [ ] Open Modulation panel → create LFO targeting hue_shift
- [ ] LFO visibly modulates parameter in main slider view
- [ ] Tweak LFO config mid-cycle → LFO continues without phase snap (tests phase continuity)
- [ ] All panels update simultaneously across multiple browser tabs
- [ ] Pi survives 10 power cycles with RO root, boots into flux, all panels accessible

---

## WR-8 — Persistent config on a read-only root

The Pi root filesystem is locked read-only by the RO/RW toggle scripts. Without
intervention, any config write (MIDI mappings, LFO state, OSC port, web port,
custom params, saved presets) silently fails on `rename(2)` and reverts on reboot.

### Verified Pi state (as of 2026-05-31)

- **OS:** Arch Linux ARM (`alarm` user, not `pi`)
- **Partitions:** `/dev/mmcblk0p2` ext4 13.6 GB at `/` · `/dev/mmcblk0p1` FAT32 1 GB at `/boot`
- **Boot partition free:** 973 MB — ample room for config and preset JSON files
- **Current config path:** `/home/alarm/.config/rustjay/` (on the root partition — will fail when RO)
- **Service file:** `/etc/systemd/system/flux.service`
- **RO/RW scripts:** plain `mount -o remount,ro /` and `mount -o remount,rw /` — no overlayfs

### Decision: use the boot partition, not a new data partition

A separate `/data` partition is the obvious solution but carries real deployment
risk: repartitioning a live SD card requires shrinking `/dev/mmcblk0p2` offline,
and a power cut during that operation corrupts the root filesystem. The payoff
does not justify the risk.

The **`/boot` partition** (`/dev/mmcblk0p1`, FAT32) already exists, is always
mounted writable by the kernel (the RO/RW scripts only remount `/`), has 973 MB
free, and is already accessed from macOS (`.Spotlight-V100` is visible in `/boot`
— the SD card is plugged into a Mac to deploy binaries). FAT32 supports
`rename(2)` atomically on Linux (the kernel FAT driver maps it to a single
directory-entry swap), so `AppSettings`'s write-to-tmp-then-rename pattern is
safe.

### Implementation — zero Rust code changes required

`AppSettings::config_path()` and `presets_dir_for()` both call
`dirs::config_dir()`, which the `dirs` crate resolves to `$XDG_CONFIG_HOME` when
set, falling back to `~/.config` otherwise. One environment variable in the
service unit redirects all writes without touching any source.

#### 8.1 Create the config directory on the boot partition

Run once on the Pi while root is still RW:

```bash
sudo mkdir -p /boot/rustjay-data
```

No `fstab` entry needed. `/boot` mounts automatically from the existing fstab
entry (`/dev/mmcblk0p1 /boot vfat defaults 0 0`).

#### 8.2 Migrate existing settings before switching

Config and presets already live at `/home/alarm/.config/rustjay/`. Copy them to
the new location before changing the service file, so no settings are lost:

```bash
# While root is RW:
cp -r /home/alarm/.config/rustjay/. /boot/rustjay-data/rustjay/
```

Verify the result:
```bash
ls /boot/rustjay-data/rustjay/
# Should show: flux.json  sputnik.json  flux/  sputnik/ (and their presets/ subdirs)
```

#### 8.3 Update the systemd service unit

The current `/etc/systemd/system/flux.service` `[Service]` section becomes:

```ini
[Service]
User=alarm
Environment=RUST_LOG=warn
Environment=XDG_CONFIG_HOME=/boot/rustjay-data
ExecStartPre=/bin/sleep 3
ExecStart=/home/alarm/flux --nogui --gles2 --drm --render-scale 0.25
Restart=on-failure
RestartSec=5
```

Then reload:
```bash
sudo systemctl daemon-reload
sudo systemctl restart flux
```

With this set, `dirs::config_dir()` → `/boot/rustjay-data`, and rustjay writes:
- Settings: `/boot/rustjay-data/rustjay/flux.json`
- Presets:  `/boot/rustjay-data/rustjay/flux/presets/*.json`

Both paths are on the FAT32 boot partition. Neither is affected by `ro`.

#### 8.4 Verify the RO root does not break saves

Add to the WR-7 runtime checklist:

- [ ] Run `ro` script to lock root read-only
- [ ] Change a MIDI mapping via the Control panel
- [ ] Run `systemctl restart flux` (simulates a power cycle without full reboot)
- [ ] Confirm the mapping survives: `cat /boot/rustjay-data/rustjay/flux.json | grep midi_mappings`
- [ ] Confirm `ls -la /boot/rustjay-data/rustjay/` shows a current `flux.json` mtime
- [ ] Run `rw` script to restore root writable when done

#### 8.5 Why not overlayfs or tmpfs + sync?

- **overlayfs:** Writes go to RAM tmpfs, discarded on reboot. Deliberately prevents
  persistence — the opposite of what we need.
- **tmpfs + flush on shutdown:** The appliance use-case involves frequent hard
  power cuts mid-set. Settings saved immediately on change (the existing
  `save_settings_requested` flag) are safer than any flush-on-exit model.

#### 8.6 Trade-offs accepted

| Property | Outcome |
|---|---|
| FAT32 file permissions | None — acceptable for a single-user appliance |
| FAT32 max file size | 4 GB — irrelevant for JSON config files |
| Boot partition free space | 973 MB; all config + presets will remain under 5 MB |
| SD card wear | Unchanged — writes happen only when settings change |
| Accessible from Mac | Yes — already done today; config visible in Finder after plugging in SD |

---

## WR-9 — Preset Panel (save / load / delete from web remote)

**Crates**: `rustjay-control`, `rustjay-engine`, `rustjay-presets`

> **Dependency on WR-8:** Preset saves go through `presets_dir_for()` which calls
> `dirs::config_dir()`. On the Pi with a read-only root, those writes fail with
> EROFS unless `XDG_CONFIG_HOME` is set to `/boot/rustjay-data` first. WR-8 must
> be deployed before WR-9 is used in production. It can be coded in parallel.

### Background: what the preset system already does

`PresetBank` reads/writes individual JSON files from `~/.config/rustjay/<app>/presets/`.
`Preset::from_state(&name, &state)` snapshots the full `EngineState`.
`bank.apply_preset(index, &mut state)` restores it. `PresetCommand` variants
(`Save`, `Load`, `Delete`, `Refresh`) are fully wired in the desktop app loop
(`process_preset_commands` in `app/commands.rs`). The GLES2 loop initialises a
`PresetBank` at startup but then drops it — the bank is not kept live for the
render loop. That gap must be fixed here.

#### 9.1 Extend `rustjay_control::web::WebCommand` with preset operations

Add to the existing `WebCommand` enum (same file as WR-1.1):

```rust
Preset(PresetWebCommand),
```

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub enum PresetWebCommand {
    /// Request an immediate broadcast of the current preset list.
    /// Useful when a panel first loads and needs to populate its list.
    List,
    /// Save current engine state as a new named preset.
    Save { name: String },
    /// Load (apply) the preset at the given index.
    Load { index: usize },
    /// Delete the preset at the given index.
    Delete { index: usize },
}
```

Add to `WebMessage` (same file):

```rust
PresetState(PresetStateJson),
```

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetStateJson {
    pub presets: Vec<PresetInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetInfo {
    pub index: usize,
    pub name:  String,
}
```

Add `send_preset_state(state: &PresetStateJson)` and a `preset_dirty: bool` flag
to `WebServer`, following the same dirty-flag pattern as WR-6.

#### 9.2 Handle preset commands in the desktop app loop

In `app/commands.rs`, inside the `while let Ok(cmd) = server.command_rx.try_recv()`
block:

```rust
WebCommand::Preset(PresetWebCommand::List) => {
    // Broadcast immediately; no state mutation.
    preset_dirty = true;
}
WebCommand::Preset(PresetWebCommand::Save { name }) => {
    // Validate name: non-empty, max 64 chars, no path separators.
    if !name.is_empty() && name.len() <= 64 && !name.contains('/') && !name.contains('\\') {
        state.preset_command = PresetCommand::Save { name };
        preset_dirty = true;
    } else {
        log::warn!("Web preset save: invalid name '{}'", name);
    }
}
WebCommand::Preset(PresetWebCommand::Load { index }) => {
    state.preset_command = PresetCommand::Load(index);
    preset_dirty = true;
}
WebCommand::Preset(PresetWebCommand::Delete { index }) => {
    state.preset_command = PresetCommand::Delete(index);
    preset_dirty = true;
}
```

> **Name validation is security-critical.** The preset name becomes part of a
> filesystem path: `presets_dir.join(format!("{}.json", name))`. A name containing
> `../` would escape the presets directory. Reject any name containing `/`, `\`,
> or `..`. Max 64 characters to prevent absurdly long filenames. The validation
> above is the minimum; add `.contains("..")` if you want belt-and-suspenders.

After processing, if `preset_dirty`, broadcast `PresetStateJson` reflecting the
current preset list. The broadcast is built from `self.preset_bank` (already
available in `App<P>`).

#### 9.3 Keep `PresetBank` live in the GLES2/DRM loop

Currently `run_drm_gles2_loop` creates a `PresetBank` at startup and drops it
at the end of the init block. Change this so the bank lives for the duration of
the loop:

```rust
// Replace the existing preset init block:
let mut preset_bank: Option<PresetBank> = presets_dir_for(&app_name).ok().map(|dir| {
    let bank = PresetBank::new(dir);
    // Populate state with preset names as before
    let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
    let slots = std::array::from_fn(|i| bank.get_slot_name(i + 1).map(|s| s.to_string()));
    let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
    state.preset_names            = names;
    state.preset_quick_slot_names = slots;
    bank
});
```

Then in the command drain section of the main loop, add:

```rust
WebCommand::Preset(PresetWebCommand::List) => {
    preset_dirty = true;
}
WebCommand::Preset(PresetWebCommand::Save { name }) => {
    if !name.is_empty() && name.len() <= 64 && !name.contains('/') && !name.contains('\\') {
        if let Some(ref mut bank) = preset_bank {
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let preset = rustjay_presets::Preset::from_state(&name, &state);
            match bank.add_preset(preset) {
                Ok(_) => {
                    state.preset_names = bank.presets.iter().map(|p| p.name.clone()).collect();
                    preset_dirty = true;
                }
                Err(e) => log::error!("Preset save failed: {e}"),
            }
        }
    }
}
WebCommand::Preset(PresetWebCommand::Load { index }) => {
    if let Some(ref mut bank) = preset_bank {
        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = bank.apply_preset(index, &mut state) {
            log::error!("Preset load failed: {e}");
        } else {
            preset_dirty = true;
        }
    }
}
WebCommand::Preset(PresetWebCommand::Delete { index }) => {
    if let Some(ref mut bank) = preset_bank {
        if let Err(e) = bank.delete_preset(index) {
            log::error!("Preset delete failed: {e}");
        } else {
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.preset_names = bank.presets.iter().map(|p| p.name.clone()).collect();
            preset_dirty = true;
        }
    }
}
```

At the end of each frame's broadcast section, if `preset_dirty`:

```rust
if preset_dirty {
    if let Some(ref bank) = preset_bank {
        let state_json = PresetStateJson {
            presets: bank.presets.iter().enumerate()
                .map(|(i, p)| PresetInfo { index: i, name: p.name.clone() })
                .collect(),
        };
        web_server.send_preset_state(&state_json);
    }
    preset_dirty = false;
}
```

#### 9.4 HTML panel — Preset Window

New file `crates/rustjay-control/src/web/presets.html`:

```
┌─ Presets ──────────────────────────────────┐
│ Save as: [__________________] [Save]       │
│                                            │
│  0  Deep Blue Session     [Load]  [✕]      │
│  1  Glitch Strobe         [Load]  [✕]      │
│  2  Warm Sunset           [Load]  [✕]      │
│                                            │
│ [Refresh]                                  │
└────────────────────────────────────────────┘
```

- **Save:** text field + button. Trims whitespace. Sends
  `Preset(Save { name })` via `POST /cmd`. Clears field on success
  (success = receiving a `PresetState` broadcast within 2 seconds).
- **Load:** button on each row. Sends `Preset(Load { index })`.
  Provides immediate visual feedback by greying the row for 500ms.
- **Delete (✕):** Sends `Preset(Delete { index })`. Asks for confirmation
  with a simple `confirm()` dialog before sending.
- **Refresh:** Sends `Preset(List)` to force a fresh broadcast. Useful if
  presets were modified on disk while the panel was open.
- **Error toast:** If no `PresetState` arrives within 2 seconds of a Save,
  show a toast: "Save failed — is the filesystem writable?". This surfaces
  the RO root failure mode that would otherwise be silent.

#### 9.5 Add route and toolbar button

Add to `create_router()` inside `protected`:
```rust
.route(&format!("/{}/presets", app_name), get(/* presets.html handler */))
```

Add to the main page toolbar:
```
[Input] [Control] [Modulation] [Presets]
```

`window.open` with token forwarded, same pattern as WR-5.3.

#### 9.6 Preset broadcast on connect

When a new WebSocket client connects, `handle_socket` already sends the full
parameter list as an initial `Params` message. Extend this to also send
`PresetState` so a freshly-opened Presets panel populates immediately without
needing to click Refresh.

The initial state snapshot is built from `WebServerState`. Add a
`preset_names: Vec<String>` field to `WebServerState` that is updated whenever
`send_preset_state` is called. `handle_socket` reads it on connect and sends
the initial `PresetState` message alongside `Params`.

---

## Out of scope (for this phase)

- **NDI input switching** — NDI source discovery is a separate protocol; defer until NDI feature is enabled on Pi (currently disabled due to `ndi = ["rustjay-io/ndi"]` not being in Pi builds).
- **Quick-slot assignment from web remote** — Eight performance quick-slots exist in `PresetBank`; assigning presets to them from the web panel is a follow-on after WR-9 ships.
- **Multi-client concurrency locks** — If two phones drag the same slider simultaneously, last-write-wins. No CRDT or OT needed at this scale.
- **Touchscreen-native UI** — Panels are desktop-browser sized; responsive CSS is enough for phones, not a native app.
- **Mobile popup fallback** — Panel windows open via `window.open`; mobile popup blocking is a known limitation, not a bug to fix.

---

## Estimation

| Task | Complexity | Risk |
|------|-----------|------|
| WR-1 Command protocol + trait extension | Medium-High | Medium — two dispatch paths, new trait method |
| WR-2 Input panel | High | Medium — GLES2 trait hook, cross-thread enum bridge |
| WR-3 Control panel | Low-Medium | Low — MIDI/OSC exists; new: change detection loop |
| WR-4 Modulation panel | Medium | Low — LFO/audio state in EngineState; Lfo reuse removes LfoConfigJson |
| WR-5 Panel UI framework | Medium | Low — vanilla JS; token-in-URL pattern is explicit |
| WR-6 State sync efficiency | Low | Low — structural channels only; param channel unchanged |
| WR-7 Verification | Medium | Medium — Pi hardware required for power-cycle test |
| WR-8 Persistent config (RO root) | Low | Low — systemd env var + mkdir, zero code change |
| WR-9 Preset panel | Medium | Low — PresetBank gap in GLES2 loop is the only new risk |

**Critical path:** WR-1 (including 1.6 trait extension) → WR-2 (GLES2 input switching). WR-8 is independent and should be deployed first since WR-9 saves to disk. WR-9 can be coded in parallel with WR-1–7 but not deployed until WR-8 is live on the Pi. Everything else can parallelise after WR-1 is merged.
