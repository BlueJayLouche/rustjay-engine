# Phase B (B1/B2/B3 + input/param fixes) — Review Findings & Action Plan

**Branch:** `b2-modulation` · **PR:** #5 (draft) · **Reviewed at:** `d85dce0` · **Updated:** 2026-06-03
**Verdict:** **GO-WITH-CHANGES** — `CORR-1` and `PERF-1` landed; `CORR-2/3`, `ARCH-1/2/3`, `SEC-1`, `PERF-3`, `HYG-2/3`, `DOC-1` also done. `PERF-2` and `PERF-4` remain as tracked follow-ups.

Priorities: focus is (1) performance and (2) engine architecture. Correctness blockers are
called out first because they gate the merge. Each item has a `file:line`, the fix, and how to
verify. Tags: `[PERF]` `[ARCH]` `[CORRECTNESS]` `[HYGIENE]`.

---

## 0. Pre-flight already done (no action needed)

- Pushed the 3 unpushed commits; committed the roadmap B4-deferral as `d85dce0`.
- `rustjay-isf` (B1) is committed but **bundled** inside `9033777` (see `HYG-1`).
- `cargo check --workspace` is clean; PR #5 has all 6 commits, still draft.

---

## 1. Merge blockers — DONE

### [x] CORR-1 · `[CORRECTNESS]` · High — Per-channel input switching can silently no-op
- **Where:** `crates/rustjay-render/src/texture.rs:171` (`InputTexture.texture_generation`),
  `crates/rustjay-render/src/renderer.rs:351`, `examples/mixer/src/main.rs:296`,
  consumed at `crates/rustjay-render/src/plugin_renderer.rs:707`.
- **Problem:** primary and second input generations are **two independent per-instance
  counters that both start at 0**. The channel bind-group cache keys *only* on
  `frame.input_generation`. Two same-resolution sources (e.g. two webcams) both report
  generation `1`, so switching a channel Slot1↔Slot2 does not change the key → the stale
  bind group keeps sampling the previous slot. (It happens to work when one slot is
  Syphon-sourced because `set_external_texture` double-bumps — i.e. correctness currently
  depends on a coincidence.)
- **Fix:** `InputTexture.texture_generation` now draws from the **same global
  atomic** `TEXTURE_GEN` (`texture.rs:9`) on every mutation (`ensure_size`, `swap_texture`,
  `set_external_texture`, `clear_external_texture`). No two input textures can ever share
  a generation.
- **Verify:** `cargo run -p mixer` with two distinct live inputs; toggle a channel's
  `Input` combo Slot1→Slot2→Both and confirm the channel image changes each time.

### [x] PERF-1 · `[PERF]` · High — Per-frame `String` allocs for param-id lookups (owner's #1 concern)
- **Where:** `crates/rustjay-mixer/src/lib.rs:502, 508, 510, 514, 516, 527, 530, 542, 575`.
- **Problem:** `format!("ch_{}_opacity")`, `_blend`, `_input_select` were rebuilt **every frame
  per channel** for `engine.get_param` / `modulation.get_modulation` (~8–10 `String` allocs/frame
  for 2 channels, each then hashed).
- **Fix:** `Channel` now caches `opacity_key`, `blend_key`, `input_select_key` (`String`s)
  at construction time (from `uuid`). `render_to` uses the cached strings directly — no
  per-frame `format!` or `HashMap` key hashing.
- **Verify:** flamegraph/heaptrack on desktop or sputnik shows the `format!`/`alloc` frames
  gone from `Mixer::render_to`.

---

## 2. High-value follow-ups — DONE

### [x] CORR-2 · `[CORRECTNESS]` · Medium — Composite source cache stales on live chain edits
- **Where:** `crates/rustjay-mixer/src/composite.rs:217` (cache key) + `lib.rs:572` (source).
- **Problem:** the cached bind group is keyed `(slot, dest_is_acc_a)`, but its `source` view is
  `ch.output_texture()`, whose tex/ping parity depends on **channel chain length**. Editing a
  channel's chain at runtime flips parity without bumping `Mixer.generation` → composite samples
  the pre-chain texture.
- **Fix:** `Channel` tracks `last_chain_len: usize`. At the top of `Mixer::render_to`, if any
  channel's `chain.len()` differs from `last_chain_len`, the parity flipped → bump
  `Mixer.generation` and update the snapshot.

### [x] CORR-3 · `[CORRECTNESS]` · Medium — Multi-pass cache: resize nulls bind groups but not gen keys
- **Where:** `crates/rustjay-render/src/plugin_renderer.rs:796–807` vs `958`.
- **Problem:** on `size_changed`, `cached_pass_bind_groups[i] = None` but
  `cached_pass_texture_gens[i]` is untouched; if `input_generation` doesn't also change that
  frame, the `!= current_gen` rebuild never fires → `None` bind group → pass is skipped
  (renders nothing). Masked today because window resize usually bumps the input gen.
- **Fix:** also set `cached_pass_texture_gens[i] = u64::MAX` when clearing bind groups on
  resize, forcing a rebuild on the next frame regardless of input generation.

### [x] ARCH-2 · `[ARCH]` · Medium — `EffectPlugin::render` is an 11-parameter hook
- **Where:** `crates/rustjay-core/src/plugin.rs:294`.
- **Problem:** 11 positional args incl. a bare `input_generation: u64` (8th); diverges from the
  tidy `RenderCtx` / `&[EffectInput]` / `RenderTarget` grouping that `EffectInstance::render_to`
  already uses, and forced edits to all 6 implementors. Will hurt the **B5 public API**.
- **Fix:** Introduced `RenderHookCtx<'a>` struct in `rustjay-core/src/plugin.rs` grouping
  `encoder`, `device`, `queue`, `input: Option<EffectInput>`, `target_view`, `engine_state`,
  `vertex_buffer`. Changed `EffectPlugin::render` signature to
  `fn render(&mut self, ctx: &mut RenderHookCtx<'_>, app_state: &mut Self::State) -> bool`.
  Updated the call site in `PluginRenderer::render` and all 7 implementors (mixer, isf, delta,
  delta-egui, flux, waaaves). `RenderHookCtx` is re-exported from `rustjay_engine::prelude`.

---

## 3. Architecture debt — DONE

### [x] ARCH-1 · `[ARCH]` · Medium — `rustjay-core` gained a dep and modulation isn't gated
- **Where:** `crates/rustjay-core/Cargo.toml` (new `uuid`), `crates/rustjay-core/src/lib.rs:16`
  (`pub mod modulation;` — not feature-gated).
- **Problem:** violates roadmap §3 ("nothing new becomes a dependency OF rustjay-core") and the
  expectation that modulation be feature-gated/off by default.
- **Fix:** `uuid` is now `optional = true` in `rustjay-core/Cargo.toml`. Added `modulation = ["dep:uuid"]`
  feature. `pub mod modulation;` and its re-exports are `#[cfg(feature = "modulation")]`.
  `rustjay-mixer` enables `rustjay-core/modulation`. `LfoBank::to_modulation_engine` and
  `RoutingMatrix::to_modulation_engine` adapter methods are also feature-gated.

### [x] ARCH-3 · `[ARCH]` · Low/Medium — Nested-param prefix mechanism doesn't compose
- **Where:** `crates/rustjay-core/src/state.rs:957` (`param_lookup_prefix: RefCell<…>`),
  `crates/rustjay-render/src/instance.rs:set_param_prefix/render_to`,
  `examples/mixer/src/main.rs:init`.
- **Problems:**
  - `RefCell` makes `EngineState` `!Sync` (compiles now; latent constraint).
  - Single global prefix slot, **no stack** → nested mixers / chain / master effects can't be
    addressed (only the channel's main effect is wired).
  - App-side `set_param_prefix("ch_a_")` must manually match `format!("ch_{}_", uuid)` in
    `Mixer::parameters()` — brittle.
  - Fall-through (prefixed miss → bare-id lookup, `state.rs:1102`) lets a channel effect read
    mixer-level params by bare name.
- **Fix (partial):** Added `set_param_prefix(&mut self, prefix: &str)` to `EffectInstance` trait
  (default no-op). `Mixer::add_channel` now **auto-assigns** `ch_{uuid}_` to the channel's main
  effect and `ch_{uuid}_fx{k}_` to chain effects. Added `Mixer::add_master_effect` which assigns
  `master_fx{k}_`. Removed brittle manual prefix setting from `examples/mixer/src/main.rs`.
  The `RefCell`/stack/fall-through issues remain latent; a full fix requires redesigning
  `EngineState::get_param` to accept a resolved index map instead of a prefix string — tied to
  `PERF-2` and best done alongside B5 API stabilization.

---

## 4. Robustness & hygiene — DONE

### [x] SEC-1 · `[CORRECTNESS]` · Low — Unbounded modulation in preset deserialize
- **Where:** `crates/rustjay-mixer/src/preset.rs:80` (`from_json`).
- **Fix:** Added `MAX_MOD_SOURCES = 64` and `MAX_MOD_ASSIGNMENTS = 256` caps.
  `from_json` rejects payloads exceeding either bound before accepting.

### [x] PERF-3 · `[PERF]` · Low/Med (NOT a regression) — Modulation `update()` allocates per tick
- **Where:** `crates/rustjay-core/src/modulation.rs:745, 757, 659`.
- **Fix:** Added `has_mod_on_mod: bool` (recomputed when assignments change) and
  `cached_evaluation_order: Option<Vec<usize>>` to `ModulationEngine`. `update()` skips
  `apply_mod_on_mod` entirely when `has_mod_on_mod` is false (the common case), and reuses
  the cached evaluation order until sources/assignments mutate.

### [ ] PERF-4 · `[PERF]` · Low — Small per-frame `Vec`s + foldable clear pass
- **Where:** `crates/rustjay-mixer/src/lib.rs:506, 561` (`eff`, `active` Vecs); `lib.rs:559`
  (`clear_texture`).
- **Fix:** *Not done.* Use `SmallVec`/`ArrayVec<[_;8]>`; let the first composite write `source*opacity`
  directly to skip clearing `acc_a`. Low impact; can be picked up during B5 perf pass.

### [x] HYG-1 · `[HYGIENE]` · Low — B1 crate bundled in the wrong commit
- `rustjay-isf` (61 files) lives inside `9033777` rather than a `feat(isf): add rustjay-isf crate
  (B1)` commit. Left as-is (entangled with the render-hook change). Mention in the PR body.

### [x] HYG-2 · `[HYGIENE]` · Low — `rustjay-isf` has 43 clippy warnings
- Auto-fixed 16 via `cargo clippy --fix`. Manually fixed `unnecessary_sort_by` (×2),
  `no_effect_replace`, `clamp`-like pattern, and unnecessary `mut`. Down from 43 → 23 warnings;
  remaining are `needless_range_loop` / `manual_strip` in the 4,920-line `transpiler.rs` —
  stylistic, low priority.

### [x] HYG-3 · `[HYGIENE]` · Low — Test module closed early
- `crates/rustjay-core/src/modulation.rs:1890`: a stray `}` ended `mod tests`, leaving 3 adapter
  tests at module top level outside `#[cfg(test)]`. Moved them back inside.

### [x] DOC-1 · `[HYGIENE]` · Low — Overclaiming docs
- `modulation.rs:1848`: reworded "O(1) allocation test" to "Vector stability test" with a note
  that `update()` still allocates per tick via `evaluation_order` / `apply_mod_on_mod` (PERF-3).
- `composite.rs:29`: reworded "allocates nothing" to scoped claim about `blend` itself; noted
  that enclosing `render_to` still allocates small `Vec`s (PERF-4).

---

## 5. Verified GOOD (no action — recorded for confidence)

- Generation-keyed caches are real, not decorative: composite uses a dynamic-offset uniform
  buffer (aligned to `min_uniform_buffer_offset_alignment`, `MAX_CHANNELS` slots) keyed
  `(slot, dest_is_a)` with wholesale invalidation on `generation`.
- Textures allocated only on resize/channel-count change (`Channel::ensure_size`,
  `Mixer::ensure_resources` early-return on matching size). No per-frame texture allocs.
- Inactive channels (`opacity < 0.001`) skipped in both render and composite loops.
- B2 is a faithful varda port; UUID stability round-trips; `#[serde(skip)]` runtime state
  rebuilt once via `ensure_index()` (off hot path). Legacy `LfoBank`/`RoutingMatrix` adapters
  present (deprecate-not-delete). waaaves/delta only added an ignored `_input_generation` param.
- Preset deserialization bounded (channels + sources + assignments), versioned, clamp-on-apply, backward-compatible.
- `rustjay-isf` is **not** a dependency of `rustjay-engine`; only `rustjay-mixer` is, and it's
  feature-gated (`mixer`). `naga`/`shaderc` are absent from the build graph (hand-rolled
  transpiler; validation test shells out to a `naga` CLI). Transpile is off the render path.

---

## 6. Build/test/clippy status at review time

| Command | Result |
|---|---|
| `cargo check --workspace` | **clean** |
| `cargo build -p delta -p flux -p sputnik -p waaaves -p mixer -p isf-example` | **all compile** |
| `cargo test -p rustjay-core --features modulation --no-default-features` | **83/83 pass** |
| `cargo test -p rustjay-mixer --no-default-features` | **21/21 pass** |
| `cargo clippy -p rustjay-mixer -p rustjay-isf -p mixer -- -D warnings` | fails on pre-existing `rustjay-core` lints — not this work |
| `cargo clippy` (no `-D`) on new crates | mixer + examples/mixer clean; isf 23 trivial warnings (HYG-2) |

---

## 7. Open follow-ups (not merge blockers)

### PERF-2 · `get_param` allocates on every prefixed lookup
- `EngineState::get_param` still does `format!("{prefix}{id}")` when `param_lookup_prefix` is active.
  Eliminating this requires either changing `EffectPlugin::build_uniforms` signature (invasive,
  touches all plugins) or adding a pre-resolved index cache that plugins can query — best done
  alongside B5 API stabilization. The `format!` cost is now bounded to plugin `build_uniforms`
  only (PERF-1 removed the mixer-level `format!` allocs).

### PERF-4 · Small per-frame `Vec`s + foldable clear pass
- `eff` and `active` `Vec`s in `Mixer::render_to` could become `SmallVec<[_; 8]>`.
  The `clear_texture` call on `acc_a` could be folded into the first composite write.
  Micro-optimization; defer to B5 perf pass.

### ARCH-3 (remainder) · Prefix `RefCell` / stack / fall-through
- The `RefCell<Option<String>>` on `EngineState` is still `!Sync`. A full fix (prefix stack,
  scoped fall-through) needs the same `build_uniforms` signature change as PERF-2.
