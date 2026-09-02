# KOVVBOJ — the one-true-app UI overhaul

Promote `examples/vjarda` to `crates/kovvboj` and replace the sidebar+tabs host
with a three-column shell: library, decks, inspector. Presentational only — the
render graph (`graph/deck.rs`, `graph/compositor.rs`) does not change.

## Decisions

| Question | Decision |
| --- | --- |
| Vessel | First-class crate `crates/kovvboj`, not an example |
| Seed | `git mv examples/vjarda crates/kovvboj` — history kept, nothing re-ported |
| Name | **KOVVBOJ** (crate, binary, window title) |
| Shell | New `AnyEguiShell` hook in `rustjay-gui`; app draws the whole frame |
| Deck model | Unchanged — one source + one chain per deck. No clip rows |
| Layout | Library left · decks centre · preview+inspector right |
| Modes | MIX / STAGE / MAP switch the centre column |
| Everything else | View-menu windows; Settings under Edit |
| Chrome | One 32px row: menus left, status pills + REC right |
| Drag & drop | Reorder in chain · move across chains · library → chain. **Not** sources |
| Undo | Structural edits only, via `Topology` snapshots |
| Selection | One global selection; any node; params live in the inspector |
| Thumbnails | One live thumb per deck row. Not per chip |

## Layout

```
┌──────────────────────────────────────────────────────────┐
│ KOVVBOJ  File Edit View Help      60fps 128bpm ●●● ●REC  │
├─────────┬──────────────────────────────┬─────────────────┤
│ LIBRARY │ [MIX] [STAGE] [MAP]          │ ┌─ PREVIEW ─┐   │
│ sources │ CH 1 ▾                       │ │           │   │
│  cam    │ □ [▶ CAM 0]─[Kaleido]─[+]    │ └───────────┘   │
│  ndi    │ CH 2 ▾                       │ INSPECTOR       │
│ shaders │ □ [▶ waves]─[Feedback]─[+]   │  Kaleido        │
│  bloom  │                              │  sides  ███░ 6  │
│  glitch │ ├─ MASTER ─────────────────  │  angle  █░░░    │
│  ↳ drag │ │ A ───█─── B  [LUT]─[+]     │  [MIDI] [LFO]   │
└─────────┴──────────────────────────────┴─────────────────┘
```

## Tab allocation

| Today | Tomorrow |
| --- | --- |
| `DeckTab` | MIX mode, centre column |
| `EffectsTab` ("Effects / Library") | Library panel, left |
| `MixerTab` | pinned MASTER row at the bottom of the deck column |
| `StageTab` | STAGE mode |
| `LedMapTab` | MAP mode |
| `OutputsTab` | View-menu window; REC gets a top-bar button |
| `SequencerTab` | View-menu window |
| `InspectorTab` (stub) | deleted — becomes the right panel |
| `MidiTab` (stub) | deleted — the engine owns MIDI |
| 11 engine builtins | View-menu windows; `Settings` → Edit menu |

## Phases

Feature branch. Every phase compiles and runs; `main` keeps working vjarda
until merge.

### P0 — the move
`git mv examples/vjarda crates/kovvboj`; rename package + binary; update
`release-apps.yml` (matrix entry `vjarda` → `kovvboj`, display `KOVVBOJ`),
`ci.yml`, workspace members, and guide references. Zero visual change.
**Done when** `cargo run -p kovvboj` is the app you have today.

### P1 — shell hook + skeleton
- `AnyEguiShell` in `rustjay-gui`: if a plugin supplies one,
  `EguiControlGui::build_ui` (`egui_control_gui.rs:299`) delegates the frame to
  it. One call site to touch: `app/events.rs:990`.
- Make builtin tab bodies reachable: `EguiControlGui::draw_builtin_tab(&mut self, ui, GuiTab)`.
- `run_with_egui_shell` beside `run_with_egui_tabs` (`rustjay-engine/src/lib.rs:167`).
- KOVVBOJ shell: merged 32px menu+status row, three columns, View menu opening
  builtins as `egui::Window`s. Existing tab bodies dropped in unmodified.
- Delete `MidiTab` and `InspectorTab`.

**Done when** every screen that worked before is reachable, in its new home.

### P2 — chain strip + inspector
- Deck card body becomes a horizontal strip of chips: `[SOURCE]─[FX]─[FX]─[+]`.
  Replaces `fx_chain_ui` (`ui/mod.rs:494`).
- Deck card keeps opacity + blend only. `deck_param_sliders` (`ui/mod.rs:380`)
  moves wholesale into the inspector.
- Global selection: channel header · deck header · source chip · FX chip.
  Nothing selected → master summary.
- Inspector param rows carry the existing MIDI-learn / LFO-map affordance
  (`map_mode_active` / `apply_param_map_overlay`).

**Done when** the deck column fits on screen without scrolling at 3 channels.

### P3 — drag & drop
`egui::dnd_drag_source` / `dnd_drop_zone` with a typed payload (egui 0.36).
- Reorder within a chain → existing `Deck::reorder_effect`.
- Cross-chain move (deck ↔ deck ↔ channel ↔ master) → re-prefix at the
  destination, then **`rekey_prefix(old, new)`** — new helper in `rustjay-core`
  rewriting modulation assignments (beside `remove_assignments_with_prefix`,
  `modulation.rs:840`) and `state.midi_mappings` (`state.rs:1022`).
- Library → chain insert at drop position. Retires the per-click native file
  dialog thread in `spawn_effect_picker` (`ui/mod.rs:433`).
- Library groups ISF by generator vs filter using the parsed ISF header
  (`inputImage` presence — see `rustjay-isf/src/effect.rs:670`).

**Done when** a mapped FX dragged to another deck keeps its knob.

### P4 — undo
`Vec<Topology>` stack, depth 32, pushed before every structural edit; undo
replays via `apply_topology` (`lib.rs:936`). Cmd+Z / Cmd+Shift+Z, Edit menu.

`ponytail:` replay rebuilds sources — undo hitches and restarts video
playback. Acceptable for structural edits; if it stops being acceptable, the
upgrade is a diff-based apply that only touches changed nodes.

Param edits are **not** undoable — they are continuous and driven by
MIDI/LFO/OSC.

### P5 — deck thumbnails
One registered wgpu texture id per deck row, re-registered when the deck's
output texture generation flips (ping-pong gotcha: a stale id shows a frozen
frame).

### P6 — modes and polish
MIX / STAGE / MAP wiring, Outputs window + top-bar REC, View-menu window
visibility persisted to the workspace, snapshot re-baseline.

## Tests

- Re-baseline affected kittest snapshots once the layout settles:
  `deck_add_source`, `outputs_projector_panel`, `sidebar_expanded`.
  One `UPDATE_SNAPSHOTS=1` run, at the end.
- Plain assert tests, no rendering, for the genuinely new logic:
  `rekey_prefix`, drag payload routing, undo push/pop.
- No pixel snapshots of drag states — every spacing tweak would re-baseline.

## Assumption to confirm

On-disk workspace stays `.varda/` (`persistence/mod.rs:111`). Zero migration,
hidden directory, nobody sees it. Say the word and it becomes `.kovvboj/` with
a one-line fallback read of `.varda/`.
