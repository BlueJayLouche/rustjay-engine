# CuePool main.rs decomposition plan

Issue: [#125](https://github.com/BlueJayLouche/rustjay-engine/issues/125)

## Goal

Split `examples/cuepool/crates/cuepool/src/main.rs` (6,337 lines) into cohesive
modules with **zero behavior change**. Mechanical moves only: no renames beyond
visibility adjustments (`pub(crate)` where a move requires it), no logic edits,
no reordering of runtime behavior, no new abstractions. `lighting_engine.rs`,
`recorder.rs`, and `mtc_follow.rs` set the precedent — flat sibling modules in
the binary crate.

Success is main.rs under roughly 2,000 lines with identical behavior, not a
perfect architecture. If a stage turns out not to move cleanly (wide visibility
fallout, tangled borrows), skip it and note why in the final summary rather
than forcing it.

## Ground rules

- Nested workspace: run everything from `examples/cuepool/`, never the repo root.
- Gates after every stage (same as CI):
  - `cargo check --workspace --all-targets`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- One commit per stage, message style `cuepool: extract <module> from main.rs`.
- Commit this plan doc first (`docs/plans/` at the repo root, already written).
- Prefer `pub(crate)` fields over adding getters. Deletion over addition.
- Do not touch `cuepool-audio`, `cuepool-video`, `cuepool-core`, `cuepool-gui`
  APIs, and do not reformat code you are not moving.

## Stages

Line numbers refer to main.rs at commit 874b364.

1. **`settings.rs`** — `AppSettings` (5671), `settings_path` (5675),
   `load_settings` (5679), `save_settings` (5688).
2. **`persist.rs`** — `spawn_autosave_thread` (5618), `emergency_save` (5700).
3. **`remote_commands.rs`** — `strip_udp_prefix` (5737), `resolve_udp_command`
   (5754), `send_udp_command` (5773), `parse_osc_command` (5793), plus their
   unit tests from `mod tests` (6020+).
4. **`cue_sequence.rs`** — `next_standby_qid` (4409), `next_after_last` (4437),
   `resolve_goto_target` (4451), plus their tests.
5. **`output_window.rs`** — `monitor_descriptor` (78), `WindowIds` (109),
   `OutputWindow` + `Drop` (153–193), `pack_size`/`unpack_size` (194–206),
   `projection_structure_changed` (397).
6. **`video_pipeline.rs`** (the big one) — `VideoMessage` (103), `OutputFrameState`
   (207–249), `VideoControl` (250), `CanvasCommand` + `coalesce_canvas_commands`
   (310–349), `win_timer` mod (364–396), `FramePacingDecision` +
   `frame_pacing_decision` (4475–4506), `update_vsync_interval` (4507),
   `video_consume_thread` (4536), `output_render_thread` (5152),
   `send_video_message` (5369), `video_decode_thread` (5395), `TimingWindow` +
   `VideoTimingWindows` (5527–5547), `timed_read_frame` (5548),
   `pixmap_decode_thread` (5565). If one file is unwieldy, a `video_pipeline/`
   directory with `mod.rs` re-exports is fine — but do not restructure the code
   itself.
7. **`cue_exec.rs`** — `ActiveCue` (115), `DelayedCue` (136), `PendingStop`
   (143), `fade_elapsed` (350), `shift_fade_start_after_pause` (354), plus the
   fade-timing tests.
8. **`impl App` split (only if clean)** — the impl block at 567–3877 is ~3,300
   lines. Move cohesive method groups into `impl App` blocks in the module
   whose concern they serve (audio-cue playback, video/output management,
   project I/O). Move a group only when it does not force wide visibility
   churn; leave anything tangled in main.rs and say so in the summary.

## Out of scope

- Lock consolidation in the frame loop (explicitly a follow-up in #125).
- Any behavior fix you spot along the way — file it in the summary instead.
- `Cargo.toml` changes, dependency changes, workflow changes.
