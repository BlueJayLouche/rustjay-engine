//! Type-erased GUI tab for egui — mirrors `AnyGuiTab`.

/// Alias for `rustjay_core::GuiTab` — the set of built-in tabs the engine renders.
pub use rustjay_core::GuiTab as BuiltinTab;

/// Type-erased GUI tab used by the egui control panel.
/// Implementors downcast app_state via `std::any::Any`.
pub trait AnyEguiTab: Send + Sync {
    /// Returns the display name of this tab.
    fn name(&self) -> &str;
    /// If Some, this tab replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> {
        None
    }
    /// Draws the tab contents.
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut rustjay_core::EngineState,
    );
}

/// Draw a float parameter slider that reads from and writes to engine state.
///
/// This is the preferred way to expose effect parameters in a custom egui tab.
/// Reading from `engine` (rather than a local state field) ensures the slider
/// reflects values set by OSC, MIDI, LFO, or any other external source.
pub fn param_slider(
    ui: &mut egui::Ui,
    engine: &mut rustjay_core::EngineState,
    id: &str,
    label: &str,
    min: f32,
    max: f32,
) {
    let mut val = engine.get_param_base(id).unwrap_or(0.0);
    if ui
        .add(egui::Slider::new(&mut val, min..=max).text(label))
        .changed()
    {
        engine.set_param_base(id, val);
    }
}

/// Draw an integer parameter slider that reads from and writes to engine state.
pub fn param_slider_int(
    ui: &mut egui::Ui,
    engine: &mut rustjay_core::EngineState,
    id: &str,
    label: &str,
    min: i32,
    max: i32,
) {
    let mut val = engine.get_param_base(id).unwrap_or(0.0).round() as i32;
    if ui
        .add(egui::Slider::new(&mut val, min..=max).text(label))
        .changed()
    {
        engine.set_param_base(id, val as f32);
    }
}
