//! ImGui + egui control GUIs for rustjay.

pub mod control_gui;
pub mod tabs;

/// The Dear ImGui *renderer backend*. Gated because `imgui-wgpu` has no wgpu 30
/// release; see [`renderer_stub`] for what stands in when it is off. The
/// `imgui` crate itself has no wgpu dependency, so the tab code is unaffected.
#[cfg(feature = "imgui-renderer")]
pub mod renderer;
#[cfg(not(feature = "imgui-renderer"))]
pub mod renderer_stub;

mod resolution_presets;

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
#[cfg(feature = "imgui-renderer")]
pub use renderer::ImGuiRenderer;
#[cfg(not(feature = "imgui-renderer"))]
pub use renderer_stub::ImGuiRenderer;

#[cfg(feature = "egui")]
pub use egui_control_gui::{apply_param_map_overlay, map_mode_active, EguiControlGui};
#[cfg(feature = "egui")]
pub use egui_renderer::EguiRenderer;
#[cfg(feature = "egui")]
pub use egui_tab::{param_slider, param_slider_int, AnyEguiShell, AnyEguiTab};
#[cfg(feature = "egui")]
pub use egui_widgets::key_color_picker;

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
