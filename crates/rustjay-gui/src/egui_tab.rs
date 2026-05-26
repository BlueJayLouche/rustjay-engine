//! Type-erased GUI tab for egui — mirrors `AnyGuiTab`.

/// Alias for `rustjay_core::GuiTab` — the set of built-in tabs the engine renders.
pub use rustjay_core::GuiTab as BuiltinTab;

/// Type-erased GUI tab used by the egui control panel.
/// Implementors downcast app_state via `std::any::Any`.
pub trait AnyEguiTab: Send + Sync {
    /// Returns the display name of this tab.
    fn name(&self) -> &str;
    /// If Some, this tab replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> { None }
    /// Draws the tab contents.
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut rustjay_core::EngineState,
    );
}
