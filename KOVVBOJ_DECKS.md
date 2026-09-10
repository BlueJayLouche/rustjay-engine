# KOVVBOJ — two decks, crossfaded through transition shaders

Prepare the next look invisibly on one deck while the other is live, then take
it over with one gesture.

Written 2026-09-10 against `main` @ 5ec392f. Branch `kovvboj-decks`.

## The model

**A deck is a `ChannelGroup`.** Not a new container type — the one already in
`rustjay-mixer`, which composites its members into its own accumulators, runs
its own FX chain, and parks the finished image in `group_out`
(`rustjay-mixer/src/lib.rs:328`, render at `:1040-1120`).

- Two **permanent** top-level groups, A and B. Created on first run, not
  deletable, never nested (`parent` is always `None`).
- Groups nest **inside** decks: `ChannelGroup.parent: Option<String>`, rendered
  deepest-first so a child's `group_out` is ready when its parent composites.
  Depth is general; a cycle guard rejects a group that is its own ancestor.
- A layer in **neither** deck blends into master as it does today — an
  always-live overlay tier that ignores the fader. Free, and useful.
- There is no "one big stack" mode any more. Single-deck working is
  "everything in A, never touch the fader".

### Why not two `Mixer`s

The first draft of this plan had `decks: [Mixer; 2]`, master FX relocated onto
`KovvbojAppState`, a `deck: u8` on `LayerDesc`, and an extract-method on
`Mixer::render_to`. All four are unnecessary: `ChannelGroup` already does every
one of those jobs. Deck membership is already `Channel.group` /
`GroupDesc.members`; master FX already runs after groups; `master_dim` already
applies to group opacity (`lib.rs:1156`); `group_out` is already the texture a
transition needs.

This also avoids re-introducing a parallel container beside `Channel` — which
is exactly the `Deck`/`Channel` incoherence deleted on 2026-09-02 (`d30dc97`,
`KOVVBOJ_UI.md` "Revision — the layer model").

## The transition

Before the master pass, deck A's and deck B's `group_out` textures go through
one ISF transition shader; the result is blended **once** into the master
accumulator, in place of blending the two decks separately. Master chain,
`master_dim` and the final blit are untouched.

**Crossfader position *is* `progress`.** One control, one uniform. A plain
dissolve is `transition_dissolve.fs` at `progress = fader`, so there is no
separate crossfade code path. Transitions are therefore symmetric and
reversible — pull the fader back and the wipe un-wipes. Correct for all nine
shipped shaders.

**Early-out:** at exactly `0.0` / `1.0`, skip the transition pass and blend the
winning deck directly. Saves a full-screen pass whenever the fader is parked,
which is most of the time. This assumes `transition(A,B,0.0) == A`, true for
the shipped set and not guaranteed for an arbitrary ISF; a one-frame pop on an
exotic shader is accepted rather than paying a pass every frame forever.

## The one engine change

`crates/kovvboj/shaders/transition_*.fs` — nine shaders (dissolve, four wipes,
push, iris, zoom, luma_key) with `startImage` / `endImage` / `progress` — have
been dead assets since the varda port. They cannot run: an ISF effect bound
every image input to the one primary texture (`rustjay-isf/src/effect.rs`), so
`startImage` and `endImage` were the same image — a transition mixed a frame
with itself and `progress` did nothing visible.

The second texture **already reaches the plugin boundary**:
`plugin_renderer.rs:604` does `let feedback = inputs.get(1)` and carries it into
`FrameInputs.feedback_view` / `feedback_sampler`. It is then dropped when
`RenderHookCtx` is built at `:658`, which sets only `input`.

So: forward those two existing fields into `RenderHookCtx`, and have
`rustjay-isf` bind them to the **second declared image input** — by
**declaration order**, not by name. Order is what ISF hosts effectively do, and
name conventions differ across corpora (`startImage`/`endImage`,
`inputImage`/`inputImage2`, `from`/`to`).

This is additive and generally useful: any two-input ISF works afterwards.

**Done** — `ca7bcb4`. `RenderHookCtx.input_b`, `IsfEffect::secondary_texture`,
and `m_second_image_input_binds_to_input_b` in `render_pixels.rs` asserting
progress 0 / 1 / 0.5. Verified non-vacuous: with the bind arm disabled,
progress 1 renders the *first* input.

**Open, for step 4:** the nine shipped `transition_*.fs` are **not in ISF
idiom** — they are pre-transpiled GLSL 450 with hand-written
`layout(set=0, binding=N)` declarations, written for varda's own pipeline.
`rustjay-isf` generates those bindings itself from an ISF JSON header. Whether
they load at all is untested; assume they need porting to the idiom
(`crates/rustjay-isf/tests/shaders/twoinput.fs` is the shape that works) and
budget for it.

## Budget

- **16 layers** (`MAX_CHANNELS`, unchanged).
- **8 groups total**, including the two decks.

Group count, not layer count, is the memory constraint.
`ChannelGroup::ensure_resources` allocates **four full-resolution textures per
group** — `acc_a`, `acc_b`, `chain_ping`, `group_out` — about **133 MB per
group at 4K RGBA8**. Eight groups is ~1 GB if all are populated, against a
current total working set of 300–515 MB.

**Allocate lazily**: a group with no members, or a muted one, allocates
nothing. An empty deck costs zero. `ensure_resources` is already called inside
the `members.is_empty() { continue; }` guard's scope, so this is a small change.

Both decks render **every frame** (they are groups; groups already do). Deck B
is live-but-unrouted. Waking a deck on demand was rejected: videos would jump
and `PHASE_TIME` generators would snap exactly as you cut to them.

## Isolation

The premise of the feature is that work on the prep deck cannot disturb the
live deck. Enforced in three places.

### Undo becomes a diff-based apply

`crates/kovvboj/src/lib.rs:688` currently reads:

> *ponytail: replaying a whole topology rebuilds every source, so an undo costs
> a hitch and restarts video playback. Acceptable for structural edits; the
> upgrade path is a diff-based apply that only touches the nodes that actually
> changed.*

Acceptable with one deck. With two it is not — an undo in the prep deck would
rebuild the live deck's decoders and hitch the screen mid-show. Build the
documented upgrade path.

**It replaces full replay entirely — it does not sit beside it.** Loading a
scene whose uuids are all new degenerates to "add everything", which is exactly
correct, so the old replay function is deleted. One path, constantly exercised,
cannot rot.

Matching is by uuid at both levels:

| Case | Action |
|---|---|
| layer uuid in both | set knobs / name / group in place; keep the instance |
| `SourceEntry` differs | route through the existing `PendingSourceSwap` path (`lib.rs:227`) — re-point live, do not rebuild the channel |
| layer only in desired | build |
| layer only in live | remove |
| fx slot uuid in both, path same | keep |
| fx slot path changed | rebuild that slot only |
| `enabled` changed | set the flag |
| order changed | reorder (uuid-stable prefixes, `move_effect`) |

Two traps:

1. **Bump `generation` whenever the channel set or order changes.**
   `add_channel` / `remove_channel` do; a diff that mutates `channels` directly
   must too, or the composite bind-group cache keyed by slot index
   (`rustjay-mixer/src/lib.rs:429`) serves stale bindings and layers render each
   other's textures. Presents as a GPU driver bug.
2. **`SourceEntry` equality is the rebuild trigger** — compare `kind`, `path`,
   `device_index` only. `id` and `name` differing must **not** force a decoder
   rebuild, so do not blanket-derive `PartialEq` and forget.

### Solo scopes to its deck subtree

`rustjay-mixer/src/lib.rs:550` — `any_solo` is
`channels.any(solo) || groups.any(solo)`, global, and the group pass gates on it
at `:1053`. Today, soloing one layer in deck A blanks every non-soloed group,
**including deck B**.

`any_solo` becomes a per-subtree computation in `effective_opacities`. An
ungrouped layer forms its own scope.

### Move, not copy

Dragging a layer between stacks is a **move**: same uuid, same source instance,
same ISF params, same modulation routings — one field changes.

**No copy verb ships.** A copy is a deep copy: fresh uuids, fresh param
prefixes, a `rekey_prefix` dance, modulation assignments *not* inherited, and a
duplicated source means a **second decoder**. Only cameras are shared (the
global `CAMERA_SESSIONS` map, `sources/camera_source.rs:54`); `ffmpeg_source`,
`hap_source`, NDI and Syphon are per-instance, and 4K software decode is the
single largest CPU cost in the app. Wanting the same clip in both decks means
adding it twice from the library, and it should feel like two decoders, because
it is. Copy stays additive, to add later with that cost understood.

## Control surface

`"crossfader"` is a **registered custom param**, min 0 / max 1. It then
inherits MIDI MAP, LFO MAP, OSC, audio-band routing and the step sequencer with
no new code. The mixer already reads it — `lib.rs:719` is
`engine.get_param("crossfader").unwrap_or(self.crossfader)` — and the key has
precedent in `preset.rs`'s legacy modulation tests.

The transition shader's own ISF inputs register under a `transition_` prefix,
mappable exactly like any FX param.

**`AutoCrossfade` / `BeatSyncCrossfade` / the sequencer write the *base*
value**; modulation adds on top in `get_param` (`rustjay-core/src/state.rs:1406`
— base, plus offset, clamped to the descriptor range). If auto-fade wrote the
final value, an LFO on the crossfader would be double-applied during a TAKE.
One owner per layer of the stack. See the `get_param` double-contribution note
in the modulation-unification work.

**TAKE is an action, not a param** — a button plus a key in `keymap.rs`, setting
`mixer.auto = Some(AutoCrossfade { … })`. Params are continuous; forcing an
event into them means inventing a trigger-on-threshold convention.
`AutoCrossfade`, `BeatSyncCrossfade`, `Easing` and `SequencerState` are all
already built and tested in `rustjay-mixer`, and have been unused since the
crossfader was deleted. This wires them up; it writes none of them.

MIDI-note binding for TAKE is later and additive.

## Persistence

`Scene` gains:

- the two deck group uuids
- `crossfader: f32`
- `transition: Option<PathBuf>`, relativized like `FxDesc.path`

Transition params ride the existing `params: HashMap<String, f32>` under the
fixed `transition_` prefix, exactly as `master_fx<uuid>_` does. No new
mechanism.

**`mixer_state` stays.** (An earlier draft deleted it as redundant with
`Topology`; that was only forced by the two-`Mixer` design. With one mixer,
`MixerState.crossfader` is the natural home for the fader value.)

`GroupDesc` gains `parent: Option<String>`.

No `TOPOLOGY_VERSION` bump and no migration: every addition is `serde(default)`
and defaults correctly. An old scene loads as a flat stack with two empty decks.
Unlike the 2026-09-02 break, nothing here is lossy — do not manufacture a
migration that is not needed.

### Savable / recallable decks

`SavedLayer` / `SavedChain` / `SavedGroup` already exist with capture,
uuid-rekeying recall and tests (`scene/mod.rs:224-470`, `:840`). Since decks are
groups, most of this is built. Two gaps:

1. **`SavedGroup` has no nested groups.** `capture` takes a flat
   `layers: Vec<LayerDesc>` (`:397`). Add `groups: Vec<GroupDesc>`, remapping
   `parent` pointers and `members` lists onto the fresh uuids. Extend
   `recalling_a_group_rekeys_everything_under_it` as the test pattern.
2. **`instantiate()` mints a fresh `group_uuid`** (`:453`) — right for dropping
   a copy into the stack, wrong for loading into a deck. Add
   **`instantiate_into(deck_uuid)`**: everything underneath gets fresh uuids as
   today, but the top-level group **keeps the target deck's uuid**, with the
   saved opacity / blend / FX applied onto it.

A deck is fixed furniture with controls bound to it. `grp_<deckA>_opacity` must
stay where the MIDI fader is mapped across every recall — otherwise the
save/recall feature is hostile to the performance surface.

With the diff-based apply, recalling into deck B rebuilds only deck B's nodes;
deck A keeps playing untouched.

## UI

Central panel splits into **two collapsible vertical stacks**, A left, B right.
Below them a strip:

```
[ preview A ]   [ ◀── crossfader ──▶  ▼ dissolve   TAKE ]   [ preview B ]
```

Master panel stays below, unchanged (`shell.rs:408`).

- Library **SOURCES** rows get `[A][B]` buttons instead of one `➕` — no
  focused-deck ambiguity.
- Library **EFFECTS** rows keep a single `➕`, appending to the *selected*
  layer's chain. `Selection` is keyed by layer uuid (`lib.rs:59-70`), so it
  already knows which deck. The asymmetry is honest: sources make layers,
  effects join a layer.
- Deck previews come from each deck's `group_out` (needs an accessor), through
  two `create_preview_texture` slots (`rustjay-gui/src/egui_renderer.rs:138`)
  copied with `copy_texture_to_texture` at preview size. kovvboj publishes the
  two deck output textures on `EngineState`; the **host** does the copy, in the
  same place it already copies master output, keeping it on the render thread.
  Ids reach the UI as `[Option<u64>; 2]`, following the existing
  `stage_preview_texture_id` pattern (`rustjay-core/src/state.rs:992`).

**Known hazard:** `KOVVBOJ_UI.md`'s unresolved nested-panel bug — centre
overpaints the inspector by ~34 px from nested deprecated `Panel::show`. The
crossfader strip is another nested panel inside `CentralPanel`, landing right on
top of it. Budget for it.

## Verification

Cheap, headless, high value:

1. **Diff-apply unit tests** — the load-bearing new logic, and pure.
   `rustjay-mixer` has a `Stub` `EffectInstance` for headless tests. Assert: a
   no-op diff rebuilds nothing; reorder preserves instances; a knob change
   preserves instances; a `SourceEntry` change swaps the source without
   rebuilding the channel.
2. **Scene round-trip** — an old scene with no `parent` / no deck uuids loads as
   a flat stack with two empty decks.
3. **One GPU pixel test for the second input** — two solid-colour inputs through
   `transition_dissolve.fs` at `progress = 0.5`, assert the midpoint colour.
   Reuses the existing harness in `rustjay-isf/tests/render_pixels.rs`, which
   already builds a `RenderHookCtx` and reads pixels back. This is the only
   thing that proves the second texture binds rather than sampling black.

**Skipped:** kittest UI snapshots. Two already fail unconditionally on macOS and
CI never sees them; deck snapshots would add noise, not signal.

**Hands-on, not automatable:** build both decks, scrub the fader through all
nine transitions, confirm nothing pops and both previews stay live.

## Build order

The first two are independently useful and mergeable before any deck work
exists.

1. **`input_b`** — forward the second input through `RenderHookCtx`, bind by
   declaration order in `rustjay-isf`, GPU pixel test. *Engine, standalone;
   unlocks every two-input ISF.*
2. **Diff-based apply** replacing full topology replay, with unit tests.
   *kovvboj, standalone; fixes undo hitching today, single deck or not.*
3. **Nested groups** — `parent` on `ChannelGroup` / `GroupDesc`, depth-first
   render, cycle guard, 8-group cap, lazy allocation, per-subtree solo.
4. **Deck roles** — two permanent top-level groups, transition pass between
   their `group_out`, `"crossfader"` registered as a param, 0/1 early-out.
5. **UI** — two stacks, crossfader strip, flanking previews, `[A][B]` library
   buttons, transition picker.
6. **TAKE** — wire `AutoCrossfade` / `BeatSyncCrossfade` / sequencer to the
   crossfader base value. Already built; this only connects it.
7. **Savable decks** — `SavedGroup` nesting, `instantiate_into(deck_uuid)`.

## Note on provenance

The first half of the design session was conducted against the
`kovvboj-layers` worktree, which is **merged and 121 commits stale**. The
`ChannelGroup` machinery that collapses most of this plan landed after it.
Read `main`.
