# rustjay-sync

Tempo sync integrations for `rustjay-engine`.

Provides optional support for two industry-standard protocols:

| Feature | Protocol | Dependency | License of dependency |
|---------|----------|------------|----------------------|
| `link`  | Ableton Link | `rusty_link` | **GPL-2.0+** |
| `prodj` | Pioneer ProDJ Link | `prodjlink-rs` | MIT |

> ⚠️ **License warning:** Enabling the `link` feature links against Ableton Link,
> which is GPL-2.0+. The resulting binary is subject to GPL terms. The base crate
> remains MIT — the GPL only applies when the `link` feature is enabled.

## Build requirements

- **For `link`:** CMake ≥ 3.14 must be installed on your system.
- **For `prodj`:** No extra system dependencies.

## Usage

Enable the features you need in your app's `Cargo.toml`:

```toml
[dependencies]
rustjay-engine = { version = "0.1", features = ["link", "prodj"] }
```

Both sync sources feed into the engine's LFO and modulation system automatically.
The engine picks the highest-priority active source:

1. Ableton Link (if enabled and peers are present)
2. ProDJ Link master deck (if enabled and a master is present)
3. Audio analysis BPM (fallback)

Plugins do not need to change — [`EngineState::effective_bpm`] and
[`EngineState::effective_beat_phase`] handle source selection transparently.

## Safety note for ProDJ Link

The `prodj` feature sends local LAN broadcast packets and may bind UDP ports
50000 and 50002. Do not run it on a production DJ network unless you understand
Pro DJ Link behavior and have operator approval.
