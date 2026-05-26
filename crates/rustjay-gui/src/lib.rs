#![warn(missing_docs)]

//! GUI crate for RustJay — ImGui control interface and wgpu renderer.
//!
//! When the `egui` feature is enabled, an egui backend is also available.

/// Control GUI with built-in tabs and device management.
pub mod control_gui;
/// wgpu-based Dear ImGui renderer.
pub mod renderer;
/// Built-in GUI tabs.
pub mod tabs;

#[cfg(feature = "egui")]
/// wgpu-based egui renderer.
pub mod egui_renderer;
#[cfg(feature = "egui")]
/// Type-erased GUI tab trait for egui.
pub mod egui_tab;
#[cfg(feature = "egui")]
/// Egui control panel.
pub mod egui_control_gui;
#[cfg(feature = "egui")]
/// Professional dark theme for egui.
pub mod egui_theme;
#[cfg(feature = "egui")]
/// Egui tab builders.
pub mod egui_tabs;

pub use control_gui::ControlGui;
pub use renderer::ImGuiRenderer;

#[cfg(feature = "egui")]
pub use egui_renderer::EguiRenderer;
#[cfg(feature = "egui")]
pub use egui_tab::AnyEguiTab;
#[cfg(feature = "egui")]
pub use egui_control_gui::EguiControlGui;

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
