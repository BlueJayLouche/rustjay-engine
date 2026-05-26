# External Control

rustjay-engine supports three protocols for controlling parameters from external devices and software: MIDI, OSC, and a web remote.

All three are active by default when the engine starts — no code changes needed.

## MIDI

### Device setup

The MIDI tab lists all connected MIDI input devices. The engine listens on all devices simultaneously — no port selection needed.

### CC learn mode

1. Open the MIDI tab in the control window
2. Click **Learn** next to a parameter
3. Move any knob or fader on your MIDI controller
4. The CC number is assigned automatically

Assignments persist in the per-app config file (`~/.config/rustjay/<app-name>.json`).

### MIDI Timecode

With the `mtc` feature enabled, the engine passively decodes MTC from any connected MIDI port. See [Tempo Sync](modulation/tempo-sync.md).

## OSC

The engine runs an OSC server on `0.0.0.0:7770` by default.

### Parameter addresses

Every declared parameter is addressable as:

```
/rustjay/<param-id>  f32
```

For example, with `ParameterDescriptor::float("intensity", ...)`:

```
/rustjay/intensity   0.75
```

Send a float value between the parameter's declared min and max.

### Checking the address

The OSC tab in the control window shows:
- The server IP and port
- A list of all active parameter paths

### Changing the port

OSC server configuration is in the per-app config file. The default port is 7770.

## Web remote

The engine runs a REST + WebSocket server that lets any browser or HTTP client read and set parameters.

### Endpoints

```
GET  /params               — list all parameters with current values
GET  /params/<id>          — get a single parameter value
POST /params/<id>  {value} — set a parameter value
WS   /ws                   — live push updates as parameters change
```

The WebSocket stream pushes a JSON message every time any parameter changes, making it easy to build reactive control surfaces.

### Using the webapp example

`examples/webapp` is a pre-built web UI that connects to the engine's web remote. Run it to get a browser-based control panel you can open from any device on the LAN:

```sh
cargo run -p webapp
# Open http://localhost:3000 (or the LAN IP shown in the Output tab)
```

The UI is built with React/TypeScript and pre-compiled in `dist/` — no Node.js needed to run it.

### QR code access

The Output tab shows a QR code pointing to the web remote's LAN address, so you can quickly open it on a phone or tablet without typing the IP.

### LAN trust mode

By default the web remote only binds to localhost. Enable LAN trust mode from the Output tab to bind to all interfaces and allow control from any device on the network.

> Only enable LAN trust mode on a trusted network — the web remote has no authentication.
