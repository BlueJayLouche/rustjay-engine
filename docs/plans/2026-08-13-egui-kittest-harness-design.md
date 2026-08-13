# Egui control UI kittest harness design

## Scope

Add headless visual and interaction coverage to the root workspace's egui control UI and Vjarda tabs. Both owning crates receive an exact `egui_kittest = "=0.34.2"` dev-dependency with the `snapshot` and `wgpu` features. Production dependencies and UI behavior remain unchanged.

## RustJay GUI harness

Reuse CuePool's deterministic harness setup: fixed dimensions, one logical pixel per point, egui's dark theme, and bundled fonts. Construct a default `EngineState`, create `EguiControlGui`, add one dummy app tab, and drive `EguiControlGui::build_ui` on every frame.

Use a 400 by 700 baseline for the fully expanded sidebar. Exercise each real section header (SIGNAL, PARAMS, CONTROL, MANAGE, and APP) and verify that a child unique to that section is absent after collapse and present after expansion. Reach Settings and Output through their painted sidebar controls before taking fixed-size snapshots, so these baselines cover host navigation as well as tab contents.

The sidebar uses painted responses instead of standard egui widgets. The test will locate the painted header or child text in the frame output, click the matching response coordinates, and assert against the next frame's painted text. This tests the shipped implementation without adding test-only hooks or changing production accessibility metadata.

## Vjarda harness

Construct `VardaAppState::default()` and `EngineState::default()` and invoke the existing `AnyEguiTab::draw` implementations directly. The default-feature run records Deck's Add Source controls and Outputs, including the recording controls and Browse button. A second run with `--features projection` records Stage with no preview texture and the projector panel in Outputs.

All Vjarda snapshots use fixed dimensions and no live engine or GPU texture. The tests do not invoke file pickers or output creation.

## Renderer fallback

Attempt PNG rendering through egui_kittest's wgpu renderer and software Vulkan. If the workspace's vendored `wgpu-hal` cannot coexist with that renderer, retain the deterministic layout and interaction assertions and explain the missing PNG path in the test file. Do not patch production GPU dependencies to force snapshot rendering.

## Verification

Commit generated PNGs below each owning crate's `tests/snapshots` directory. Confirm `cargo tree --edges normal` excludes `egui_kittest`, then run workspace check and clippy, the rustjay-gui egui tests, Vjarda's default tests, and Vjarda's projection tests. Run every Cargo command with `cuepool-linux-check.env` loaded.
