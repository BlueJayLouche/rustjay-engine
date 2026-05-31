# External Control

rustjay-engine supports three protocols for controlling parameters from external devices and software: MIDI, OSC, and a web remote. All three are active by default when the engine starts — no code changes needed.

---

## Web remote

The engine runs a WebSocket + HTTP server (default port **8081**) that provides a full browser-based control surface. Any phone or laptop on the same network can open it without installing anything.

### Access

On startup, the engine logs a URL with an embedded bearer token:

```
Web server ready:
  Local:   http://127.0.0.1:8081/flux?token=a1b2c3d4...
  Network: http://192.168.1.42:8081/flux?token=a1b2c3d4...
```

Open the network URL in any browser. The token is regenerated each launch.

### LAN trust mode

For headless operation (Pi, rack unit) where typing a token is inconvenient, enable LAN trust mode in the app config:

```json
{
  "web_host": "0.0.0.0",
  "web_port": 8081,
  "web_lan_trust": true
}
```

With LAN trust active, all requests from the local subnet pass through without authentication. Disable it on untrusted networks.

### Panels

Five panels are available from the toolbar on the main page:

| Panel | URL path | Purpose |
|---|---|---|
| Main | `/<app>` | Parameter sliders — all declared parameters |
| Input | `/<app>/input` | V4L2 webcam selection, resolution, restart |
| Control | `/<app>/control` | OSC enable/port, MIDI device connect, MIDI learn, mapping list |
| Modulation | `/<app>/modulation` | LFO configuration, tap tempo, audio reactivity display |
| Presets | `/<app>/presets` | Save / load / delete named presets |

Each panel opens in its own tab. All panels share a single WebSocket connection and receive live state updates.

### Command protocol

Panels write commands via `POST /<app>/cmd` with a JSON body. The outer `type` field selects the subsystem; the `action` field selects the operation within it.

**Parameter value:**
```json
{"type":"set","id":"flux/flow_scale","value":1.8}
```

**Input (webcam):**
```json
{"type":"input","action":"select_device","index":0,"width":720,"height":576,"fps":25}
{"type":"input","action":"stop"}
{"type":"input","action":"refresh_devices"}
```

**OSC / MIDI control:**
```json
{"type":"control","action":"osc","enabled":true}
{"type":"control","action":"osc_set_port","port":9001}
{"type":"control","action":"midi_learn","param_id":"flux/flow_scale"}
{"type":"control","action":"midi_learn_cancel"}
{"type":"control","action":"midi_unlearn","cc":14,"channel":0}
{"type":"control","action":"midi_select_device","device":"Arturia BeatStep"}
{"type":"control","action":"midi_disconnect"}
```

**LFO / modulation:**
```json
{"type":"modulation","action":"lfo_enable","slot":0,"enabled":true}
{"type":"modulation","action":"lfo_set","slot":0,"config":{"index":0,"enabled":true,"target":"HueShift","waveform":"Sine","amplitude":0.5,"tempo_sync":true,"division":4,"rate":1.0,"phase_offset":0.0}}
{"type":"modulation","action":"tap_tempo"}
```

**Presets:**
```json
{"type":"preset","action":"save","name":"Deep Blue Session"}
{"type":"preset","action":"load","index":0}
{"type":"preset","action":"delete","index":0}
{"type":"preset","action":"list"}
```

### State broadcasts

The WebSocket pushes JSON messages to all connected panels. The `type` field identifies the message:

| `type` | Payload fields | Sent when |
|---|---|---|
| `params` | `params: [{id, name, category, min, max, value, step}]` | On connect — full initial state |
| `update` | `id, value` | Each time a parameter value changes |
| `input_state` | `devices, active_index, active_name, width, height, fps` | After any input command |
| `control_state` | `osc_enabled, osc_port, midi_enabled, midi_selected_device, midi_devices, midi_mappings, midi_learn_active` | After any control change or MIDI mapping update |
| `modulation_state` | `lfos, audio_routes, audio_routing_enabled, bpm, tap_tempo_info` | After any LFO or audio routing change |
| `preset_state` | `presets: [{index, name}]` | After save/load/delete, and on connect |

---

## MIDI

### Device setup

Connect a USB MIDI controller. Available devices appear in the **Control** panel's *MIDI Devices* section. Click **Connect** to start receiving.

Alternatively, connect from the GUI MIDI tab (desktop) or the web Control panel (headless).

### CC learn mode

**Desktop (GUI):** open the MIDI tab, click **Learn** next to a parameter, move a knob or fader.

**Headless (web Control panel):**
1. Open `http://<pi>:8081/<app>/control`
2. Scroll to the *Parameters* section
3. Click **Learn** next to any parameter
4. Move a CC on the controller — the mapping appears immediately

The **LEARNING…** badge at the top of the Parameters section shows when learn mode is active. Click **Cancel** to abort.

### Persistence

MIDI device selection and all CC mappings are saved automatically to the per-app config file every time a mapping changes. They are restored on the next launch, including reconnecting to the same device if it is present.

---

## OSC

The engine runs an OSC server on `0.0.0.0:9000` by default (configurable from the Control panel or config file).

### Parameter addresses

Every declared parameter is addressable as:

```
/<app-base-address>/<category>/<param-id>  f32
```

For example, with the default base address `/rustjay` and a parameter declared as:

```rust
ParameterDescriptor::float("flow_scale", ...)
    .category(ParamCategory::Flux)
```

The address is `/rustjay/flux/flow_scale`. Send a float value in the parameter's `[min, max]` range.

The OSC tab in the desktop GUI (or a plain `cat /proc/<pid>/net/udp6` on the Pi) shows the active port.

### Changing the port

From the **Control** web panel, update the *Listen Port* field. The server restarts on the new port immediately. The new port is saved to config.

Or set it directly in the config JSON:
```json
{
  "osc": { "host": "0.0.0.0", "port": 9001, "enabled": true }
}
```
