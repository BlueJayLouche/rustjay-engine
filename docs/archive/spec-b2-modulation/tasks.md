# B2 — Tasks

**Critical path:** T01 → T02 → T03 → T04 → T05 → T06 → T07

---

## T01 — Workspace bootstrap
- [ ] Add `uuid = { version = "1", features = ["v4"] }` to `[workspace.dependencies]`.
- [ ] Add `uuid` to `crates/rustjay-core/Cargo.toml`.
- [ ] `cargo check -p rustjay-core` passes.

## T02 — Port modulation types
- [ ] Create `crates/rustjay-core/src/modulation.rs`.
- [ ] Port enums: `LFOWaveform`, `AudioReactMode`, `AudioBandPreset`, `ADSRStage`, `StepInterpolation`.
- [ ] Port structs: `ParamModulation`, `ModulationSourceEntry`, `AudioSourceValues`, `AudioValues`.
- [ ] Port `ModulationSource` enum with `calculate()`, `config_eq()`, `gate_on()`, `gate_off()`, constructors.
- [ ] Use `u32` instead of Varda's `AudioSourceId`; use `uuid::Uuid::new_v4()` for ID generation.
- [ ] `cargo check -p rustjay-core` passes.

## T03 — Port ModulationEngine
- [ ] Port `ModulationEngine` struct and all methods.
- [ ] Ensure `uuid_to_idx` is `#[serde(skip)]` and rebuilt on structural changes.
- [ ] Ensure `prev_values`, `current_values`, `prev_time` are `#[serde(skip)]`.
- [ ] Port `evaluation_order()` with MAX_MOD_DEPTH = 4.
- [ ] Port `apply_mod_on_mod()` for all source types.
- [ ] `cargo check -p rustjay-core` passes.

## T04 — Unit tests
- [ ] Port all varda modulation tests into `modulation.rs` `#[cfg(test)]` module.
- [ ] Add serialize round-trip test (REQ-06.1).
- [ ] Add O(1) allocation test: assert vector capacity stable after first update (REQ-06.2).
- [ ] Add mod-on-mod cycle / deep chain tests (REQ-06.4).
- [ ] `cargo test -p rustjay-core` passes.

## T05 — Wire into rustjay-core
- [ ] Add `pub mod modulation;` to `rustjay-core/src/lib.rs`.
- [ ] Add re-exports for all public modulation types.
- [ ] Verify no name collisions with existing `lfo`/`routing` re-exports.
- [ ] `cargo check -p rustjay-core` passes.

## T06 — Adapter layer (B2.2)
- [ ] Add `LfoBank::to_modulation_sources(&self) -> Vec<ModulationSourceEntry>`.
- [ ] Add `LfoBank::to_modulation_engine(&self, bpm: f32) -> ModulationEngine`.
- [ ] Add `RoutingMatrix::to_modulation_sources(&self) -> Vec<ModulationSourceEntry>`.
- [ ] Add `RoutingMatrix::to_modulation_engine(&self) -> ModulationEngine`.
- [ ] Write adapter unit tests verifying conversion of 8 default LFOs + 2 default routes.
- [ ] `cargo test -p rustjay-core` passes.

## T07 — Workspace verification
- [ ] `cargo check --workspace` clean.
- [ ] `cargo test --workspace` clean.
- [ ] `cargo clippy -p rustjay-core` clean (ignore pre-existing warnings in other crates).
- [ ] `cargo build -p delta` compiles and runs unchanged.
- [ ] `cargo build -p waaaves` compiles and runs unchanged.
- [ ] `cargo build -p flux` compiles unchanged.
- [ ] `cargo build -p sputnik` compiles unchanged.
- [ ] `cargo build -p mixer` compiles unchanged.

## T08 — Documentation & commit
- [ ] Ensure all public items have doc comments (`#![warn(missing_docs)]`).
- [ ] Final commit message ending with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
