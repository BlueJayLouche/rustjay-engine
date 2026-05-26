# Custom Tabs

Custom tabs let you add your own panel to the control window. Use them when you need controls that don't fit the standard parameter-slider model: toggle groups, waveform previews, complex layouts, or widgets that act on multiple parameters at once.

## Implementing `AnyGuiTab`

```rust
use rustjay_engine::prelude::*;

struct MyTab;

impl AnyGuiTab for MyTab {
    fn name(&self) -> &str { "My Effect" }

    fn draw(
        &mut self,
        ui:         &imgui::Ui,
        app_state:  &mut dyn std::any::Any,
        engine:     &mut EngineState,
    ) {
        // Downcast app_state to your concrete type
        let state = app_state
            .downcast_mut::<MyState>()
            .expect("MyTab: wrong state type");

        // Draw ImGui widgets
        ui.slider_config("Intensity", 0.0_f32, 1.0_f32)
            .build(&mut state.intensity);

        if ui.button("Reset") {
            state.intensity = 0.0;
        }
    }
}
```

Register it with `run_with_tabs`:

```rust
fn main() -> anyhow::Result<()> {
    rustjay_engine::run_with_tabs(MyEffect, vec![Box::new(MyTab)])
}
```

The tab appears at the right end of the tab bar.

## Replacing a built-in tab

If your effect has its own colour controls and you want one custom "Effect" tab instead of the built-in Color tab:

```rust
impl AnyGuiTab for MyTab {
    fn name(&self) -> &str { "Effect" }

    fn replaces(&self) -> Option<BuiltinTab> {
        Some(BuiltinTab::Color)  // hides the built-in Color tab
    }

    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn std::any::Any, _engine: &mut EngineState) {
        let state = app_state.downcast_mut::<MyState>().unwrap();
        ui.slider_config("Hue Shift", -180.0_f32, 180.0_f32).build(&mut state.hue_shift);
        ui.slider_config("Saturation", 0.0_f32, 2.0_f32).build(&mut state.saturation);
    }
}
```

## ImGui widget reference

A quick reference of frequently used ImGui widgets:

```rust
// Sliders
ui.slider_config("Label", min, max).build(&mut state.value);

// Drag (finer control, no visible range)
imgui::Drag::new("Label").speed(0.01).build(ui, &mut state.value);

// Checkbox
ui.checkbox("Enabled", &mut state.enabled);

// Combo (dropdown)
let items = ["Replace", "Add", "Multiply", "Screen"];
let mut current = state.blend_mode as usize;
if ui.combo_simple_string("Blend Mode", &mut current, &items) {
    state.blend_mode = current as u32;
}

// Button
if ui.button("Randomise") { /* ... */ }

// Colour picker (returns [f32; 4])
ui.color_edit4_config("Tint", imgui::ColorEditFlags::NO_ALPHA)
  .build(&mut state.tint);

// Text
ui.text(format!("BPM: {:.1}", engine.effective_bpm()));

// Separator + header
ui.separator();
ui.text_colored([1.0, 0.8, 0.2, 1.0], "-- Section --");
```

## Using the egui backend

If you're building with the `egui` feature, use `EguiAnyTab` instead and draw with `egui::Ui`. The pattern is identical — implement the trait, downcast `app_state`, draw widgets — but the widget API differs.

```rust
// Cargo.toml: rustjay-engine = { ..., features = ["egui"] }

use rustjay_engine::prelude::*;

struct MyEguiTab;

impl EguiAnyTab for MyEguiTab {
    fn name(&self) -> &str { "Effect" }

    fn draw(
        &mut self,
        ui:        &egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine:    &mut EngineState,
    ) {
        let state = app_state.downcast_mut::<MyState>().unwrap();
        ui.add(egui::Slider::new(&mut state.intensity, 0.0..=1.0).text("Intensity"));
    }
}

fn main() -> anyhow::Result<()> {
    rustjay_engine::run_with_tabs(MyEffect, vec![Box::new(MyEguiTab)])
}
```

See `examples/delta-egui` for a complete egui-backend example.
