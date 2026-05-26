//! HUD / instrument-panel dark theme for egui — matches the web remote control aesthetic.
//!
//! Drop-in replacement for the previous Ableton/Resolume-inspired theme. The colour
//! constants in the `colors` sub-module keep their *names* (ACCENT_CYAN, ACCENT_AMBER, …)
//! so existing tabs compile unchanged — they just resolve to the new HUD palette.
//!
//! Aesthetic notes:
//! - Square corners (0px radius) everywhere — the HUD look uses crisp orthogonal edges.
//! - Monospace as the default text style. Proportional is still available for body copy.
//! - Hairline borders (1px, low-opacity white) instead of solid grey strokes.
//! - Amber is the primary signal colour; green = online; red = alert; cool = secondary.

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Visuals};

/// Apply the HUD theme to the egui context.
pub fn apply_professional_theme(ctx: &Context) {
    let mut style = Style::default();

    // ── Palette (mirrors the web --css variables) ────────────────────────────
    let bg          = Color32::from_rgb(0x07, 0x09, 0x0b); // --bg
    let surface     = Color32::from_rgb(0x0c, 0x10, 0x14); // --surface
    let surface_2   = Color32::from_rgb(0x11, 0x16, 0x1c); // --surface-2
    let hair        = Color32::from_rgba_premultiplied(15, 15, 16, 16);  // ~rgba(255,255,255,0.06)
    let hair_2      = Color32::from_rgba_premultiplied(30, 30, 32, 32);  // ~rgba(255,255,255,0.12)
    let hair_3      = Color32::from_rgba_premultiplied(56, 56, 60, 56);  // ~rgba(255,255,255,0.22)

    let ink         = Color32::from_rgb(0xe8, 0xeb, 0xee); // --ink
    let ink_2       = Color32::from_rgb(0xaa, 0xb1, 0xb9); // --ink-2
    let _ink_3      = Color32::from_rgb(0x6a, 0x72, 0x80); // --ink-3
    let _ink_4      = Color32::from_rgb(0x3a, 0x40, 0x48); // --ink-4

    let amber       = Color32::from_rgb(0xe8, 0xa0, 0x4a); // primary signal
    let amber_dim   = Color32::from_rgb(0x8a, 0x5e, 0x2b);
    let _signal     = Color32::from_rgb(0x46, 0xd4, 0x86); // online
    let _alert      = Color32::from_rgb(0xe8, 0x63, 0x4a); // alert
    let _cool       = Color32::from_rgb(0x7e, 0xc6, 0xd6); // secondary

    // ── Global visuals ───────────────────────────────────────────────────────
    style.visuals = Visuals::dark();
    style.visuals.override_text_color = Some(ink);
    style.visuals.panel_fill = bg;
    style.visuals.window_fill = surface;
    style.visuals.window_stroke = Stroke::new(1.0, hair_2);

    // Non-interactive (labels, group backgrounds)
    style.visuals.widgets.noninteractive.bg_fill = surface;
    style.visuals.widgets.noninteractive.weak_bg_fill = surface;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, hair);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ink);

    // Inactive (buttons at rest, sliders track)
    style.visuals.widgets.inactive.bg_fill = surface_2;
    style.visuals.widgets.inactive.weak_bg_fill = surface_2;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, hair_2);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, ink_2);

    // Hovered
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x18, 0x1f, 0x27);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x18, 0x1f, 0x27);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, hair_3);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, ink);

    // Active (pressed, dragged)
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(0x22, 0x2c, 0x37);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(0x22, 0x2c, 0x37);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, amber);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, ink);

    // Open (combobox dropdown, etc.)
    style.visuals.widgets.open.bg_fill = Color32::from_rgb(0x18, 0x1f, 0x27);
    style.visuals.widgets.open.weak_bg_fill = Color32::from_rgb(0x18, 0x1f, 0x27);
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, amber_dim);
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, ink);

    // Selection (highlighted slider fill, selected list rows)
    style.visuals.selection.bg_fill = amber;
    style.visuals.selection.stroke = Stroke::new(1.0, amber);

    style.visuals.hyperlink_color = amber;
    style.visuals.faint_bg_color = bg;
    style.visuals.extreme_bg_color = bg;
    style.visuals.code_bg_color = surface_2;
    style.visuals.warn_fg_color = amber;
    style.visuals.error_fg_color = Color32::from_rgb(0xe8, 0x63, 0x4a);

    // Square corners — the defining structural choice of the HUD look
    style.visuals.window_corner_radius = CornerRadius::ZERO;
    style.visuals.menu_corner_radius = CornerRadius::ZERO;
    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
    style.visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;
    style.visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
    style.visuals.widgets.active.corner_radius = CornerRadius::ZERO;
    style.visuals.widgets.open.corner_radius = CornerRadius::ZERO;
    style.visuals.window_shadow = egui::epaint::Shadow::NONE;
    style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    style.visuals.collapsing_header_frame = true;
    style.visuals.indent_has_left_vline = true;

    // ── Typography ───────────────────────────────────────────────────────────
    // Monospace as the default everywhere — gives the instrument feel.
    style.text_styles = [
        (TextStyle::Heading,  FontId::new(15.0, FontFamily::Monospace)),
        (TextStyle::Body,     FontId::new(12.5, FontFamily::Monospace)),
        (TextStyle::Monospace,FontId::new(12.5, FontFamily::Monospace)),
        (TextStyle::Button,   FontId::new(12.0, FontFamily::Monospace)),
        (TextStyle::Small,    FontId::new(10.5, FontFamily::Monospace)),
    ].into();

    // ── Spacing ──────────────────────────────────────────────────────────────
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.indent = 16.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.handle_min_length = 24.0;
    style.spacing.slider_width = 200.0;

    ctx.set_global_style(style);
}

/// Colour constants — names preserved from the previous theme for compatibility.
/// `ACCENT_CYAN` now resolves to the HUD amber so existing call sites
/// (`.color(ACCENT_CYAN)` for headings, highlights, etc.) automatically pick up the new look.
pub mod colors {
    use egui::Color32;

    // ── Primary palette ──────────────────────────────────────────────────────
    pub const BG:          Color32 = Color32::from_rgb(0x07, 0x09, 0x0b);
    pub const SURFACE:     Color32 = Color32::from_rgb(0x0c, 0x10, 0x14);
    pub const SURFACE_2:   Color32 = Color32::from_rgb(0x11, 0x16, 0x1c);
    pub const HAIR:        Color32 = Color32::from_rgba_premultiplied(15, 15, 16, 16);
    pub const HAIR_2:      Color32 = Color32::from_rgba_premultiplied(30, 30, 32, 32);
    pub const HAIR_3:      Color32 = Color32::from_rgba_premultiplied(56, 56, 60, 56);

    pub const INK:         Color32 = Color32::from_rgb(0xe8, 0xeb, 0xee);
    pub const INK_2:       Color32 = Color32::from_rgb(0xaa, 0xb1, 0xb9);
    pub const INK_3:       Color32 = Color32::from_rgb(0x6a, 0x72, 0x80);
    pub const INK_4:       Color32 = Color32::from_rgb(0x3a, 0x40, 0x48);

    pub const AMBER:       Color32 = Color32::from_rgb(0xe8, 0xa0, 0x4a);
    pub const AMBER_DIM:   Color32 = Color32::from_rgb(0x8a, 0x5e, 0x2b);
    pub const SIGNAL:      Color32 = Color32::from_rgb(0x46, 0xd4, 0x86);
    pub const ALERT:       Color32 = Color32::from_rgb(0xe8, 0x63, 0x4a);
    pub const COOL:        Color32 = Color32::from_rgb(0x7e, 0xc6, 0xd6);

    // ── Back-compat aliases ──────────────────────────────────────────────────
    // Existing tabs reference these names — remap them all onto the HUD palette
    // so the rest of the codebase doesn't need to change.
    pub const ACCENT_CYAN:    Color32 = AMBER;       // primary highlight = amber now
    pub const ACCENT_AMBER:   Color32 = AMBER;
    pub const ACCENT_GREEN:   Color32 = SIGNAL;
    pub const ACCENT_RED:     Color32 = ALERT;
    pub const TEXT_PRIMARY:   Color32 = INK;
    pub const TEXT_SECONDARY: Color32 = INK_3;
    pub const BG_WIDGET:      Color32 = SURFACE_2;
    pub const BG_HOVER:       Color32 = Color32::from_rgb(0x18, 0x1f, 0x27);
    pub const BG_ACTIVE:      Color32 = Color32::from_rgb(0x22, 0x2c, 0x37);
    pub const BORDER:         Color32 = Color32::from_rgba_premultiplied(30, 30, 32, 32);

    /// FFT band colours — restained to harmonise with the HUD palette
    /// (warmer overall, narrower hue range, keep them distinguishable).
    pub const FFT_BANDS: [Color32; 8] = [
        Color32::from_rgb(0xe8, 0x63, 0x4a), // Sub      — alert red
        Color32::from_rgb(0xe8, 0x83, 0x3a), // Bass
        Color32::from_rgb(0xe8, 0xa0, 0x4a), // Lo Mid   — amber
        Color32::from_rgb(0xd9, 0xc2, 0x5a), // Mid
        Color32::from_rgb(0x9c, 0xd4, 0x6a), // Hi Mid
        Color32::from_rgb(0x46, 0xd4, 0x86), // High     — signal green
        Color32::from_rgb(0x7e, 0xc6, 0xd6), // V.High   — cool
        Color32::from_rgb(0xa8, 0xa0, 0xd8), // Pres
    ];
}
