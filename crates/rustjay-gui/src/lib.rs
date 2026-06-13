//! ImGui + egui control GUIs for rustjay.

pub mod control_gui;
pub mod renderer;
pub mod tabs;

#[cfg(feature = "egui")]
pub mod egui_control_gui;
#[cfg(feature = "egui")]
pub mod egui_renderer;
#[cfg(feature = "egui")]
pub mod egui_tab;
#[cfg(feature = "egui")]
pub mod egui_tabs;
#[cfg(feature = "egui")]
pub mod egui_theme;
#[cfg(feature = "egui")]
pub mod egui_widgets;

pub use control_gui::ControlGui;
pub use renderer::ImGuiRenderer;

#[cfg(feature = "egui")]
pub use egui_control_gui::EguiControlGui;
#[cfg(feature = "egui")]
pub use egui_renderer::EguiRenderer;
#[cfg(feature = "egui")]
pub use egui_tab::{param_slider, param_slider_int, AnyEguiTab};

/// Type-erased GUI tab used by [`ControlGui`].
/// Implementors downcast `app_state` via [`std::any::Any`].
pub trait AnyGuiTab: Send + Sync {
    fn name(&self) -> &str;
    /// If `Some`, replaces the named built-in tab instead of appending.
    fn replaces(&self) -> Option<BuiltinTab> {
        None
    }
    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut rustjay_core::EngineState,
    );
}

pub use rustjay_core::GuiTab as BuiltinTab;
