<div class="rj-hero">
  <div class="rj-hero-inner">
    <div class="rj-logo">rustjay<span class="rj-logo-accent">-engine</span></div>
    <p class="rj-tagline">A Rust framework for building real-time VJ applications.<br>
    You bring the shader. The engine handles everything else.</p>
    <div class="rj-cta">
      <a href="getting-started/index.html" class="rj-btn rj-btn-primary">Get Started →</a>
      <a href="https://github.com/BlueJayLouche/rustjay-engine" class="rj-btn rj-btn-ghost">GitHub ↗</a>
    </div>
  </div>
</div>

<div class="rj-features">
  <div class="rj-feature">
    <div class="rj-feature-icon">▣</div>
    <h3>GPU Rendering</h3>
    <p>wgpu on Metal, Vulkan, and DX12. Single-pass, multi-pass feedback, mesh displacement, and compute shaders.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">◉</div>
    <h3>Audio Analysis</h3>
    <p>8-band FFT, beat detection, BPM estimation, and an audio-to-parameter routing matrix — all live.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">〜</div>
    <h3>LFO Modulation</h3>
    <p>3 independent LFO banks with 5 waveforms each. Tempo-sync to BPM, beat-phase lock, per-parameter depth.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">⬡</div>
    <h3>Video I/O</h3>
    <p>Webcam, NDI, Syphon (macOS), Spout (Windows), and V4L2 (Linux) — for input and output.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">◈</div>
    <h3>MIDI · OSC · Web</h3>
    <p>MIDI CC learn, OSC server, and a REST + WebSocket web remote. Control from anything on the network.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">♩</div>
    <h3>Tempo Sync</h3>
    <p>Ableton Link, Pioneer ProDJ Link, and MIDI Timecode. Lock to the DJ or the DAW.</p>
  </div>
</div>

---

## What is rustjay-engine?

**rustjay-engine** is a Cargo workspace of focused crates that handles all the infrastructure of a live visual performance tool — GPU rendering, video I/O, audio analysis, parameter modulation, and network control — so you can focus on the one thing that makes your effect unique: the shader.

When you implement the `EffectPlugin` trait and call `rustjay_engine::run()`, you get a complete dual-window application:

- **Output window** — fullscreen GPU-rendered output
- **Control window** — a tabbed panel with built-in sections for input sources, audio analysis, LFO modulation, MIDI mapping, output routing, and presets

## Running the examples

```sh
git clone https://github.com/BlueJayLouche/rustjay-engine
cd rustjay-engine
cargo run -p template    # HSB colour — the simplest effect
cargo run -p delta       # RGB delay / motion extraction
cargo run -p waaaves     # multi-pass feedback pipeline
cargo run -p sputnik     # mesh displacement (Rutt-Etra style)
```

## How to use this guide

Start with [Installation](installation.md), then work through [Your First Effect](getting-started/index.html). After that, the chapters are largely independent — jump to whatever is relevant to what you're building.
