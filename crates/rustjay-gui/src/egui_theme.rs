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

// The palette constants below are self-documenting by name.
#![allow(missing_docs)]

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Visuals};

/// Apply the HUD theme to the egui context.
pub fn apply_professional_theme(ctx: &Context) {
    let mut style = Style::default();

    // ── Palette ──────────────────────────────────────────────────────────────
    // Read from the active palette so a preset swap repaints egui's own
    // widget visuals, not just the hand-painted HUD chrome.
    let p = palette();
    let (bg, surface, surface_2) = (p.bg, p.surface, p.surface_2);
    let (hair, hair_2, hair_3) = (p.hair, p.hair_2, p.hair_3);
    let (ink, ink_2) = (p.ink, p.ink_2);
    let (amber, amber_dim) = (p.accent, p.accent_dim);

    // ── Global visuals ───────────────────────────────────────────────────────
    style.visuals = Visuals::dark();
    style.visuals.override_text_color = Some(ink);
    style.visuals.panel_fill = bg;
    style.visuals.window_fill = surface;
    style.visuals.window_stroke = Stroke::new(1.0_f32, hair_2);

    // Non-interactive (labels, group backgrounds)
    style.visuals.widgets.noninteractive.bg_fill = surface;
    style.visuals.widgets.noninteractive.weak_bg_fill = surface;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, hair);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, ink);

    // Inactive (buttons at rest, sliders track)
    style.visuals.widgets.inactive.bg_fill = surface_2;
    style.visuals.widgets.inactive.weak_bg_fill = surface_2;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, hair_2);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, ink_2);

    // Hovered
    style.visuals.widgets.hovered.bg_fill = p.bg_hover;
    style.visuals.widgets.hovered.weak_bg_fill = p.bg_hover;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, hair_3);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, ink);

    // Active (pressed, dragged)
    style.visuals.widgets.active.bg_fill = p.bg_active;
    style.visuals.widgets.active.weak_bg_fill = p.bg_active;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, amber);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, ink);

    // Open (combobox dropdown, etc.)
    style.visuals.widgets.open.bg_fill = p.bg_hover;
    style.visuals.widgets.open.weak_bg_fill = p.bg_hover;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, amber_dim);
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, ink);

    // Selection (highlighted slider fill, selected list rows)
    style.visuals.selection.bg_fill = amber;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, amber);

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
        (TextStyle::Heading, FontId::new(15.0, FontFamily::Monospace)),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Monospace)),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
        (TextStyle::Button, FontId::new(12.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(10.5, FontFamily::Monospace)),
    ]
    .into();

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
/// Every colour role the GUI paints with.
///
/// Roles, not colours: `signal` means "this is online", not "this is green".
/// A preset assigns colours to roles; call sites ask for the role. Swapping a
/// preset therefore repaints the whole UI without touching a single call site.
///
/// The base roles (`bg`, `surface*`, `hair*`, `ink*`) are addressable even
/// though the shipped presets currently only differ in their accents — a future
/// preset can restyle the ground without any further plumbing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub hair: Color32,
    pub hair_2: Color32,
    pub hair_3: Color32,

    pub ink: Color32,
    pub ink_2: Color32,
    pub ink_3: Color32,
    pub ink_4: Color32,

    /// Primary accent — selection, active strokes, the wordmark.
    pub accent: Color32,
    /// Dimmed primary, for open-but-not-active chrome.
    pub accent_dim: Color32,
    /// "Online" / healthy.
    pub signal: Color32,
    /// "Something is wrong".
    pub alert: Color32,
    /// Secondary accent.
    pub cool: Color32,

    pub bg_hover: Color32,
    pub bg_active: Color32,

    /// Eight FFT band colours, low to high.
    pub fft_bands: [Color32; 8],
}

impl Palette {
    /// The amber HUD the engine has always shipped. Default for every app.
    pub const HUD_AMBER: Self = Self {
        bg: Color32::from_rgb(0x07, 0x09, 0x0b),
        surface: Color32::from_rgb(0x0c, 0x10, 0x14),
        surface_2: Color32::from_rgb(0x11, 0x16, 0x1c),
        hair: Color32::from_rgba_premultiplied(15, 15, 16, 16),
        hair_2: Color32::from_rgba_premultiplied(30, 30, 32, 32),
        hair_3: Color32::from_rgba_premultiplied(56, 56, 60, 56),
        ink: Color32::from_rgb(0xe8, 0xeb, 0xee),
        ink_2: Color32::from_rgb(0xaa, 0xb1, 0xb9),
        ink_3: Color32::from_rgb(0x6a, 0x72, 0x80),
        ink_4: Color32::from_rgb(0x3a, 0x40, 0x48),
        accent: Color32::from_rgb(0xe8, 0xa0, 0x4a),
        accent_dim: Color32::from_rgb(0x8a, 0x5e, 0x2b),
        signal: Color32::from_rgb(0x46, 0xd4, 0x86),
        alert: Color32::from_rgb(0xe8, 0x63, 0x4a),
        cool: Color32::from_rgb(0x7e, 0xc6, 0xd6),
        bg_hover: Color32::from_rgb(0x18, 0x1f, 0x27),
        bg_active: Color32::from_rgb(0x22, 0x2c, 0x37),
        fft_bands: [
            Color32::from_rgb(0xe8, 0x63, 0x4a),
            Color32::from_rgb(0xe8, 0x83, 0x3a),
            Color32::from_rgb(0xe8, 0xa0, 0x4a),
            Color32::from_rgb(0xd9, 0xc2, 0x5a),
            Color32::from_rgb(0x9c, 0xd4, 0x6a),
            Color32::from_rgb(0x46, 0xd4, 0x86),
            Color32::from_rgb(0x7e, 0xc6, 0xd6),
            Color32::from_rgb(0xa8, 0xa0, 0xd8),
        ],
    };

    /// KOVVBOJ. Same dark ground, saturated accents.
    pub const KOVVBOJ: Self = Self {
        accent: Color32::from_rgb(0xFA, 0x32, 0xC1),     // magenta
        accent_dim: Color32::from_rgb(0xA5, 0x5E, 0x58), // muted rose
        signal: Color32::from_rgb(0x32, 0xFA, 0x8B),     // green
        alert: Color32::from_rgb(0xF7, 0x40, 0x31),      // red
        cool: Color32::from_rgb(0x32, 0xF8, 0xFA),       // cyan
        fft_bands: [
            Color32::from_rgb(0xF7, 0x40, 0x31),
            Color32::from_rgb(0xF8, 0x39, 0x79),
            Color32::from_rgb(0xFA, 0x32, 0xC1),
            Color32::from_rgb(0xC5, 0x62, 0xDD),
            Color32::from_rgb(0x8F, 0x93, 0xF9),
            Color32::from_rgb(0x32, 0xF8, 0xFA),
            Color32::from_rgb(0x32, 0xFA, 0xC2),
            Color32::from_rgb(0x32, 0xFA, 0x8B),
        ],
        ..Self::HUD_AMBER
    };

    /// Every shipped preset, as (id, display name). `id` is what gets persisted.
    pub const PRESETS: [(&'static str, &'static str); 2] =
        [("hud_amber", "HUD Amber"), ("kovvboj", "KOVVBOJ")];

    /// Look a preset up by its persisted id. Unknown ids fall back to the
    /// default rather than failing — a workspace naming a preset this build
    /// does not have should still open.
    pub fn by_id(id: &str) -> Self {
        match id {
            "kovvboj" => Self::KOVVBOJ,
            _ => Self::HUD_AMBER,
        }
    }
}

/// The palette every `colors::*` accessor reads.
static ACTIVE: std::sync::RwLock<Palette> = std::sync::RwLock::new(Palette::HUD_AMBER);

/// The palette currently in force.
pub fn palette() -> Palette {
    *ACTIVE.read().unwrap_or_else(|e| e.into_inner())
}

/// Swap the palette. Takes effect on the next repaint; call
/// [`apply_professional_theme`] afterwards so egui's own `Visuals` follow.
pub fn set_palette(p: Palette) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = p;
}

/// Colour roles, resolved against the active palette.
///
/// These were `const`s until the palette became swappable. They are functions
/// now so a preset change repaints without a restart; call sites read
/// `colors::amber()` rather than `colors::AMBER`.
pub mod colors {
    use super::palette;
    use egui::Color32;

    // ── Primary palette ──────────────────────────────────────────────────────
    pub fn bg() -> Color32 { palette().bg }
    pub fn surface() -> Color32 { palette().surface }
    pub fn surface_2() -> Color32 { palette().surface_2 }
    pub fn hair() -> Color32 { palette().hair }
    pub fn hair_2() -> Color32 { palette().hair_2 }
    pub fn hair_3() -> Color32 { palette().hair_3 }

    pub fn ink() -> Color32 { palette().ink }
    pub fn ink_2() -> Color32 { palette().ink_2 }
    pub fn ink_3() -> Color32 { palette().ink_3 }
    pub fn ink_4() -> Color32 { palette().ink_4 }

    pub fn amber() -> Color32 { palette().accent }
    pub fn amber_dim() -> Color32 { palette().accent_dim }
    pub fn signal() -> Color32 { palette().signal }
    pub fn alert() -> Color32 { palette().alert }
    pub fn cool() -> Color32 { palette().cool }

    // ── Role aliases kept from the pre-palette theme ─────────────────────────
    pub fn accent_cyan() -> Color32 { palette().accent }
    pub fn accent_amber() -> Color32 { palette().accent }
    pub fn accent_green() -> Color32 { palette().signal }
    pub fn accent_red() -> Color32 { palette().alert }
    pub fn text_primary() -> Color32 { palette().ink }
    pub fn text_secondary() -> Color32 { palette().ink_3 }
    pub fn bg_widget() -> Color32 { palette().surface_2 }
    pub fn bg_hover() -> Color32 { palette().bg_hover }
    pub fn bg_active() -> Color32 { palette().bg_active }
    pub fn border() -> Color32 { palette().hair_2 }

    /// FFT band colours, low to high.
    pub fn fft_bands() -> [Color32; 8] { palette().fft_bands }
}
