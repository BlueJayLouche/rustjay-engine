# delta-egui — egui Backend Edition

`examples/delta-egui` is the same RGB delay effect as [delta](delta.md) — same `DeltaEffect`, same `FrameHistory` ring buffer, same `DeltaState` and `DeltaUniforms` — built with the **egui** control backend instead of ImGui.

```sh
cargo run -p delta-egui
```

Read the [delta page](delta.md) for everything about the effect itself. This page covers only what changes when switching to egui.

## Enabling the egui feature

```toml
# Cargo.toml
[dependencies]
rustjay-engine = { git = "...", features = ["egui"] }
```

## Entry point

```rust
// ImGui version:
rustjay_engine::run_with_tabs(DeltaEffect::default(), vec![Box::new(MotionTab)])

// egui version:
rustjay_engine::run_with_egui_tabs(DeltaEffect::default(), vec![Box::new(MotionTab)])
```

One function name difference. The rest of the plugin — `EffectPlugin` impl, state, uniforms, `render()` — is identical.

## Implementing a tab with egui

Implement `AnyEguiTab` instead of `AnyGuiTab`. The trait signature is the same, but `draw` receives `&mut egui::Ui` instead of `&imgui::Ui`:

```rust
// ImGui
impl AnyGuiTab for MotionTab {
    fn name(&self) -> &str { "Motion" }
    fn replaces(&self) -> Option<GuiTab> { Some(GuiTab::Motion) }

    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn Any, engine: &mut EngineState) {
        ui.slider_config("Intensity", 0.0_f32, 1.0_f32).build(&mut state.intensity);
    }
}

// egui
impl AnyEguiTab for MotionTab {
    fn name(&self) -> &str { "Motion" }
    fn replaces(&self) -> Option<GuiTab> { Some(GuiTab::Motion) }

    fn draw(&mut self, ui: &mut egui::Ui, app_state: &mut dyn Any, engine: &mut EngineState) {
        param_slider(ui, engine, "intensity", "Intensity", 0.0, 1.0);
    }
}
```

## `param_slider` helpers

The prelude exports two egui-specific helpers that keep the engine's parameter registry in sync without boilerplate:

```rust
// Float slider — reads get_param_base, writes set_param_base on change
param_slider(ui, engine, "intensity", "Intensity", 0.0, 1.0);

// Integer slider
param_slider_int(ui, engine, "red_delay", "Red", 0, 16);
```

These are equivalent to:

```rust
let mut val = engine.get_param_base("intensity").unwrap_or(1.0);
if ui.add(egui::Slider::new(&mut val, 0.0..=1.0).text("Intensity")).changed() {
    engine.set_param_base("intensity", val);
}
```

For types not covered by the helpers (bool checkboxes, enum combo boxes), call `engine.get_param_base` / `engine.set_param_base` directly:

```rust
// Bool
let mut grayscale = engine.get_param_base("grayscale_input").unwrap_or(1.0) > 0.5;
if ui.checkbox(&mut grayscale, "Grayscale Input").changed() {
    engine.set_param_base("grayscale_input", if grayscale { 1.0 } else { 0.0 });
}

// Enum / combo
let mut idx = engine.get_param_base("blend_mode").unwrap_or(0.0).round() as usize;
egui::ComboBox::from_id_salt("blend_mode")
    .selected_text(blend_names[idx])
    .show_ui(ui, |ui| {
        for (i, name) in blend_names.iter().enumerate() {
            if ui.selectable_label(idx == i, *name).clicked() { idx = i; }
        }
    });
engine.set_param_base("blend_mode", idx as f32);
```

## egui widget quick reference

```rust
// Headings and labels
ui.heading("Section title");
ui.label(egui::RichText::new("Bold label").strong());
ui.separator();

// Sliders (param_slider covers the common case)
ui.add(egui::Slider::new(&mut val, min..=max).text("Label"));

// Checkbox
ui.checkbox(&mut flag, "Label");

// Combo box
egui::ComboBox::from_id_salt("unique_id")
    .selected_text(current_label)
    .show_ui(ui, |ui| { /* selectable_label calls */ });

// Horizontal layout
ui.horizontal(|ui| { ui.label("Key:"); ui.label("Value"); });

// Group scoping (prevents id collisions across tabs)
ui.push_id("my_tab", |ui| { /* widgets */ });
```
