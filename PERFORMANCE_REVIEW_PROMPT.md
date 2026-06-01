# Performance Review Prompt

## Context

You are reviewing the **RustJay Engine**, a Rust-based real-time graphics/audio engine built on wgpu/Metal. The codebase lives in `/Users/ac/developer/rust/rustjay-engine`.

A profiling run has just been completed on the `sputnik` example (mesh + LFO, no video input, 120fps ProMotion display, 30-second idle capture). We need a second pair of eyes on the findings and code-level recommendations.

## Artifacts to Review

Please read these files **before** forming conclusions:

1. **`PERF_FINDINGS.md`** — Prior baseline profile (2026-05-19) with root-cause analysis and 3 tracked tasks (P-1, P-2, P-3).
2. **`PROFILE_REPORT_2026-06-01.md`** — Fresh profile from today with updated numbers and thread-level breakdown.
3. **`sputnik_sample.txt`** — Raw macOS `sample` output (1.5 MB, 1ms intervals, fully symbolicated). Use this to verify any claims about hot functions.
4. **`crates/rustjay-engine/src/app/events.rs`** — Main event loop; contains the `about_to_wait` handler and `TARGET_FRAME_DUR` constant.
5. **`crates/rustjay-render/src/plugin_renderer.rs`** — The render path suspected of per-frame `write_buffer` staging allocations.
6. **`crates/rustjay-render/src/texture.rs`** — `InputTexture::update` path that fires `Queue::write_texture` every frame.

## What We Need From You

### 1. Validate the findings
- Do the numbers in `PROFILE_REPORT_2026-06-01.md` accurately reflect the raw `sample` data?
- Are our attribution of blame (vsync vs. staging buffer vs. texture upload) correct?
- Is there **any hot path we missed** that appears in the sample but wasn't called out?

### 2. Code-level recommendations for each bottleneck

**Bottleneck A: Frame-rate cap is compile-time constant (Task P-1)**
- `events.rs` hard-codes `TARGET_FRAME_DUR` to 120fps.
- We want to make this runtime-configurable via `EngineConfig` or an `App` field.
- Please suggest the **minimal, cleanest implementation** that doesn't break existing examples (`sputnik`, `delta`, `waaaves`, `flux`).

**Bottleneck B: Per-frame staging buffer allocation in `plugin_renderer.rs` (Task P-2)**
- Every frame, `queue.write_buffer(...)` allocates a new `StagingBuffer` for uniform data.
- The proposed fix is `wgpu::util::StagingBelt`.
- Please review `plugin_renderer.rs` and tell us:
  - Where exactly should `StagingBelt` live? (on `WgpuEngine`, `PluginRenderer`, elsewhere?)
  - What chunk size is appropriate?
  - Where should `belt.finish()` and `belt.recall()` be called relative to `queue.submit()`?
  - Are there lifetime or async issues with the current code structure?

**Bottleneck C: Per-frame texture upload in `InputTexture::update`**
- `InputTexture::update` calls `Queue::write_texture` every frame even when no new video frame has arrived.
- Please review `texture.rs` and suggest:
  - Whether we can early-exit when the input hasn't changed.
  - If not, whether a persistent staging buffer or `StagingBelt` can be reused here too.

### 3. Risk / trade-off analysis
- For each recommendation, what is the **risk of regression**?
- Which change gives the best CPU reduction per unit of effort?
- Are there any wgpu version constraints (currently wgpu 29.0) that would block `StagingBelt` usage?

### 4. Concrete next steps
- Provide a ranked list of changes with file paths, line numbers, and pseudocode or real Rust snippets where helpful.
- If a change is unsafe or complex, flag it explicitly.

## Constraints

- **Do not** change test logic or break the public API unless necessary.
- Follow the existing Rust style in the codebase (check `crates/rustjay-render/src/` and `crates/rustjay-engine/src/` for patterns).
- The engine must still build and run on macOS Metal and eventually cross-compile to Linux/GLES (Raspberry Pi).
- Keep changes minimal — this is a performance pass, not a refactor.

## Output Format

Please structure your response as:

```markdown
## Validation
[Confirm or challenge each finding with evidence from the sample data]

## Ranked Recommendations
### 1. [Title] — Effort: X, Expected Gain: Y%
[File, line numbers, proposed change, risks]

### 2. ...

## Missed Opportunities
[Any hot paths or optimizations not covered in the existing reports]

## Appendix: Supporting Data
[Quotes from sputnik_sample.txt or code snippets backing your conclusions]
```
