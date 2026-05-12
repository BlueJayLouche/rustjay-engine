#![warn(missing_docs)]

//! GUI crate for RustJay — ImGui control interface and wgpu renderer.

/// Control GUI with built-in tabs and device management.
pub mod control_gui;
/// wgpu-based Dear ImGui renderer.
pub mod renderer;
/// Built-in GUI tabs.
pub mod tabs;

pub use control_gui::ControlGui;
pub use renderer::ImGuiRenderer;

/// Type-erased GUI tab used by ControlGui.
/// Implementors downcast app_state via std::any::Any.
pub trait AnyGuiTab: Send + Sync {
    /// Returns the display name of this tab.
    fn name(&self) -> &str;
    /// If Some, this tab replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> { None }
    /// Draws the tab contents.
    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut rustjay_core::EngineState,
    );
}

/// Alias for `rustjay_core::GuiTab` — the set of built-in tabs the engine renders.
/// Used as the return type of `AnyGuiTab::replaces()`.
pub use rustjay_core::GuiTab as BuiltinTab;
