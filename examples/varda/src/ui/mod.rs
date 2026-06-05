//! GUI — egui tabs replacing Varda's 11 panels.
//!
//! Pattern: implement `AnyEguiTab`, `replaces()` a built-in where appropriate,
//! and drive parameters through `engine.get_param*` / `set_param_base`.
//!
//! See VARDA_PORT.md §5 and `examples/delta-egui`.

/// Mixer tab — crossfader, per-channel opacity, master FX.
pub struct MixerTab;

/// Deck tab — source picker, opacity/blend/scaling, deck FX.
pub struct DeckTab;

/// Effects / Library tab — drag-add from registry, reorder.
pub struct EffectsTab;

/// Modulation tab — LFO/audio/ADSR/step assignment + chaining graph.
pub struct ModulationTab;

/// Sequencer tab — transition sequences.
pub struct SequencerTab;

/// MIDI tab — device select, learn/unlearn, mapping table.
pub struct MidiTab;

/// Stage tab — 2D surface editor, warp handles, import.
pub struct StageTab;

/// Outputs tab — window/display/NDI/stream/record assignment.
pub struct OutputsTab;

/// Inspector tab — context panel for selected node.
pub struct InspectorTab;
