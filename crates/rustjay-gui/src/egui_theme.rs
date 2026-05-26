//! Professional dark theme for egui — inspired by Ableton Live, Resolume Arena, and TouchDesigner.

use egui::{Color32, Context, CornerRadius, Stroke, Style, Visuals};

/// Apply the professional dark VJ theme to the egui context.
pub fn apply_professional_theme(ctx: &Context) {
    let mut style = Style::default();

    // ── Colour palette ───────────────────────────────────────────────────────
    let bg_dark = Color32::from_rgb(0x1a, 0x1a, 0x1a);
    let bg_panel = Color32::from_rgb(0x24, 0x24, 0x24);
    let bg_widget = Color32::from_rgb(0x2e, 0x2e, 0x2e);
    let bg_hover = Color32::from_rgb(0x38, 0x38, 0x38);
    let bg_active = Color32::from_rgb(0x44, 0x44, 0x44);
    let border = Color32::from_rgb(0x33, 0x33, 0x33);
    let text_primary = Color32::from_rgb(0xe0, 0xe0, 0xe0);
    let _text_secondary = Color32::from_rgb(0x88, 0x88, 0x88);
    let accent_cyan = Color32::from_rgb(0x00, 0xbc, 0xd4);
    let accent_cyan_dim = Color32::from_rgb(0x00, 0x8c, 0x9e);
    let _accent_green = Color32::from_rgb(0x4c, 0xaf, 0x50);
    let accent_amber = Color32::from_rgb(0xff, 0x98, 0x00);
    let accent_red = Color32::from_rgb(0xf4, 0x43, 0x36);

    // ── Global visuals ───────────────────────────────────────────────────────
    style.visuals = Visuals::dark();
    style.visuals.override_text_color = Some(text_primary);
    style.visuals.panel_fill = bg_panel;
    style.visuals.window_fill = bg_panel;
    style.visuals.window_stroke = Stroke::new(1.0, border);
    style.visuals.widgets.noninteractive.bg_fill = bg_panel;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.widgets.inactive.bg_fill = bg_widget;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.widgets.inactive.weak_bg_fill = bg_widget;
    style.visuals.widgets.hovered.bg_fill = bg_hover;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.widgets.hovered.weak_bg_fill = bg_hover;
    style.visuals.widgets.active.bg_fill = bg_active;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.widgets.active.weak_bg_fill = bg_active;
    style.visuals.widgets.open.bg_fill = bg_active;
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.selection.bg_fill = accent_cyan_dim;
    style.visuals.selection.stroke = Stroke::new(1.0, accent_cyan);
    style.visuals.hyperlink_color = accent_cyan;
    style.visuals.faint_bg_color = bg_dark;
    style.visuals.extreme_bg_color = bg_dark;
    style.visuals.code_bg_color = bg_widget;
    style.visuals.warn_fg_color = accent_amber;
    style.visuals.error_fg_color = accent_red;
    style.visuals.window_corner_radius = CornerRadius::same(6);
    style.visuals.window_shadow = egui::epaint::Shadow::NONE;
    style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    style.visuals.collapsing_header_frame = true;
    style.visuals.indent_has_left_vline = true;

    // ── Widget corner radii ──────────────────────────────────────────────────
    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(4);
    style.visuals.widgets.open.corner_radius = CornerRadius::same(4);

    // ── Spacing ──────────────────────────────────────────────────────────────
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.indent = 16.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.handle_min_length = 24.0;

    ctx.set_global_style(style);
}

/// Convenience colour constants for tab builders.
pub mod colors {
    use egui::Color32;

    pub const ACCENT_CYAN: Color32 = Color32::from_rgb(0x00, 0xbc, 0xd4);
    pub const ACCENT_GREEN: Color32 = Color32::from_rgb(0x4c, 0xaf, 0x50);
    pub const ACCENT_AMBER: Color32 = Color32::from_rgb(0xff, 0x98, 0x00);
    pub const ACCENT_RED: Color32 = Color32::from_rgb(0xf4, 0x43, 0x36);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xe0, 0xe0, 0xe0);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
    pub const BG_WIDGET: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
    pub const BG_HOVER: Color32 = Color32::from_rgb(0x38, 0x38, 0x38);
    pub const BG_ACTIVE: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
    pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);

    /// FFT band colours (same as ImGui audio tab).
    pub const FFT_BANDS: [Color32; 8] = [
        Color32::from_rgb(0xcc, 0x1a, 0x1a), // Sub
        Color32::from_rgb(0xe6, 0x73, 0x0d), // Bass
        Color32::from_rgb(0xd9, 0xbf, 0x0d), // Lo Mid
        Color32::from_rgb(0x66, 0xd9, 0x1a), // Mid
        Color32::from_rgb(0x0d, 0xd9, 0x59), // Hi Mid
        Color32::from_rgb(0x0d, 0xcc, 0xe6), // High
        Color32::from_rgb(0x40, 0x66, 0xf2), // V.High
        Color32::from_rgb(0xbf, 0x33, 0xf2), // Pres
    ];
}
