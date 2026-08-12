# rustjay-lighting

DMX lighting output for Rust — sACN (E1.31) and Art-Net over UDP.

Pure CPU + networking: no GPU, no windowing, no audio. Build a `DmxFrame`
(universe → 512 channel bytes), hand it to a `DmxSender`, and it paces
transmission on a background thread through a `DmxTransport`
(`SacnTransport` or `ArtNetTransport`).

```rust,no_run
use rustjay_lighting::{DmxFrame, DmxSender, SacnTransport, Dest};

let transport = SacnTransport::new(Dest::Multicast, 100, "my-app").unwrap();
let sender = DmxSender::spawn(Box::new(transport), 44.0);

let mut frame = DmxFrame::new();
let u = frame.universe_mut(1);
u[0] = 255; // fixture 1 red
sender.submit(frame);
```

## What's in the box

- **Transports** — sACN (E1.31) multicast/unicast and Art-Net
  broadcast/unicast, with paced transmission (`DmxSender`) at a configurable
  refresh rate.
- **Receive** — `DmxReceiver` listens for sACN/Art-Net input (monitoring,
  merge, record).
- **Fixtures** — `FixtureProfile`/`FixtureLook` color pipeline (RGB/RGBW,
  amber/UV, master dimmer, white modes, 16-bit pan/tilt), built-in profiles,
  and patch overlap detection.
- **Pixel mapping** — `demux_tile`/`ScanOrder` helpers for mapping a pixel
  buffer onto LED tiles and strips.
- **Show recording** — `RecWriter`/`read_rec` capture DMX to a compact
  `.dmxrec` file; `ShowPlayer` and `PunchRecorder` play back and punch-in
  over live input.

## Example

A runnable smoke test streams a moving rainbow across six RGB fixtures:

```sh
cargo run -p rustjay-lighting --example sacn_smoke            # sACN multicast
cargo run -p rustjay-lighting --example sacn_smoke -- artnet  # Art-Net broadcast
```

Verify with sACNView or any Art-Net monitor (QLC+, Resolume, …).

## Origin

Extracted from [rustjay-engine](https://github.com/BlueJayLouche/rustjay-engine),
where it drives the lighting subsystem for the engine and the CuePool show
controller. Design notes live in the repository's guide
(`guide/src/lighting.md`).

## License

MIT
