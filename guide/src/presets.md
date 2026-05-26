# Presets

Presets are full-state snapshots. A saved preset captures the current engine state (LFO settings, audio routing, MIDI mappings, input/output config) plus your effect's `State` struct.

## Saving and loading

Use the **Presets** tab in the control window. Presets are stored as JSON files in:

```
~/.config/rustjay/<app-name>/presets/
```

Each effect's presets are isolated by `app_name()`. Running `template` and `delta` from the same machine gives them separate preset banks.

## Quick-slots

Eight quick-slot buttons appear at the top of the Presets tab. Press `Shift+F1`–`Shift+F8` on the output window to recall them instantly, without switching to the control window.

Save a preset to a quick-slot: right-click the slot button and choose **Save here**, or drag-and-drop from the preset list.

## Including plugin state

By default, presets only save engine state (LFO, audio, MIDI, etc.). To save your effect's own state — extra textures, ring-buffer configuration, anything beyond `EngineState` — implement three preset hooks:

```rust
impl EffectPlugin for MyEffect {
    // ...

    fn serialize_preset_state(&self, state: &MyState) -> Option<String> {
        // Return a JSON string, or None to skip
        serde_json::to_string(state).ok()
    }

    fn deserialize_preset_state(&self, data: &str, state: &mut MyState) {
        if let Ok(loaded) = serde_json::from_str::<MyState>(data) {
            *state = loaded;
        }
    }

    fn on_preset_applied(&self, state: &mut MyState, engine: &mut EngineState) {
        // Called after both engine state and plugin state are restored.
        // Use to push any required commands back to the engine.
        // e.g. engine.input_commands.push(InputCommand::SetDevice(...));
    }
}
```

Because `State` already derives `serde::Serialize + DeserializeOwned`, `serde_json::to_string` / `from_str` are the standard approach.

## State initialisation vs presets

Preset loading calls `deserialize_preset_state()` and then `on_preset_applied()`. The initial state at launch comes from `default_state()`. These are separate paths — don't rely on `on_preset_applied()` for startup initialisation.

## File format

Preset files are plain JSON. You can hand-edit them, version-control them, or share them. The top-level keys are:

```json
{
  "engine": { /* EngineState fields */ },
  "plugin": "{ /* your serialized State as a JSON string */ }"
}
```
