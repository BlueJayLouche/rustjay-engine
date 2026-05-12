pub mod control_gui;
pub mod renderer;
pub mod tabs;

pub use control_gui::ControlGui;
pub use renderer::ImGuiRenderer;

/// Type-erased GUI tab used by ControlGui.
/// Implementors downcast app_state via std::any::Any.
pub trait AnyGuiTab: Send + Sync {
    fn name(&self) -> &str;
    /// If Some, this tab replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> { None }
    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut rustjay_core::EngineState,
    );
}

/// The set of built-in tabs the engine renders by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTab {
    Input, Color, Audio, Output, Presets, Midi, Osc, Web, Settings,
}
