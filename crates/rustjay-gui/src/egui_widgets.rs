//! HUD widget helpers — corner brackets, section headers, segmented toggles, slider cards.
//!
//! These mirror the visual primitives from the web remote control:
//!   ┌───────────┐
//!   │ corners + │   on framed surfaces
//!   └───────────┘
//!   ▌ SECTION · 03 CH · 01/04                — section headers w/ amber tick + counter
//!   [ OFF │ ON ]                              — segmented toggles
//!   ──•─────────  with tick marks underneath  — sliders with min/mid/max scale labels
//!
//! All helpers are pure egui — no extra deps. They opt-in: existing tabs keep working,
//! refactor them at your own pace to use these.

#![allow(missing_docs)] // widget enum/helpers are self-describing by name

use crate::egui_theme::colors::*;
use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

// ─────────────────────────────────────────────────────────────────────────────
// Frame / decoration
// ─────────────────────────────────────────────────────────────────────────────

/// Paint amber corner brackets on the outside of a rect. Length = 10px each leg.
/// Call after you've drawn the framed content so the brackets sit on top of the border.
pub fn corner_brackets(ui: &Ui, rect: Rect, color: Color32) {
    let p = ui.painter();
    let l = 10.0;
    let s = Stroke::new(1.0, color);
    let r = rect;
    // top-left
    p.line_segment([r.left_top(), r.left_top() + Vec2::new(l, 0.0)], s);
    p.line_segment([r.left_top(), r.left_top() + Vec2::new(0.0, l)], s);
    // top-right
    p.line_segment([r.right_top(), r.right_top() - Vec2::new(l, 0.0)], s);
    p.line_segment([r.right_top(), r.right_top() + Vec2::new(0.0, l)], s);
    // bottom-left
    p.line_segment([r.left_bottom(), r.left_bottom() + Vec2::new(l, 0.0)], s);
    p.line_segment([r.left_bottom(), r.left_bottom() - Vec2::new(0.0, l)], s);
    // bottom-right
    p.line_segment([r.right_bottom(), r.right_bottom() - Vec2::new(l, 0.0)], s);
    p.line_segment([r.right_bottom(), r.right_bottom() - Vec2::new(0.0, l)], s);
}

/// A framed HUD panel — square 1px border + optional amber corner brackets.
/// Returns the inner Rect (after padding) so callers can lay out inside it.
pub fn hud_frame<R>(
    ui: &mut Ui,
    brackets: bool,
    pad: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, HAIR_2))
        .inner_margin(egui::Margin::same(pad as i8))
        .show(ui, |ui| {
            let r = add_contents(ui);
            if brackets {
                corner_brackets(ui, ui.min_rect(), AMBER);
            }
            r
        })
        .inner
}

// ─────────────────────────────────────────────────────────────────────────────
// Section header — ▌ TITLE · 03 CH · 01/04
// ─────────────────────────────────────────────────────────────────────────────

/// HUD section header: amber tick glyph, uppercase title, dashed rule, optional counter.
/// Use instead of `ui.heading(...)` / `ui.separator()`.
pub fn hud_section_header(ui: &mut Ui, title: &str, counter: Option<&str>) {
    ui.add_space(8.0);
    let row_height = 18.0;
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::hover());
    let painter = ui.painter();

    // Amber tick glyph ▌
    painter.rect_filled(
        Rect::from_min_size(
            rect.left_top() + Vec2::new(0.0, 2.0),
            Vec2::new(3.0, row_height - 4.0),
        ),
        0.0,
        AMBER,
    );

    // Title (uppercase, letterspaced visually via tracking)
    let title_pos = rect.left_top() + Vec2::new(10.0, row_height / 2.0);
    let title_galley = painter.layout_no_wrap(title.to_uppercase(), FontId::monospace(11.0), INK_2);
    painter.galley(
        Pos2::new(title_pos.x, title_pos.y - title_galley.size().y / 2.0),
        title_galley.clone(),
        INK_2,
    );

    // Counter on the right (e.g. "03 CH · 01/04")
    let counter_w = if let Some(c) = counter {
        let g = painter.layout_no_wrap(c.to_string(), FontId::monospace(10.0), INK_4);
        let w = g.size().x;
        painter.galley(
            Pos2::new(rect.right() - w, rect.center().y - g.size().y / 2.0),
            g,
            INK_4,
        );
        w + 12.0
    } else {
        0.0
    };

    // Dashed rule between title and counter
    let rule_left = rect.left() + 10.0 + title_galley.size().x + 10.0;
    let rule_right = rect.right() - counter_w;
    if rule_right > rule_left + 8.0 {
        let y = rect.center().y;
        let mut x = rule_left;
        let dash = 4.0;
        let gap = 3.0;
        while x < rule_right {
            let end = (x + dash).min(rule_right);
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(end, y)],
                Stroke::new(1.0, HAIR_2),
            );
            x = end + gap;
        }
    }
    ui.add_space(6.0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Segmented toggle — [ OFF │ ON ]
// ─────────────────────────────────────────────────────────────────────────────

/// A two-segment OFF/ON toggle. Returns true when the value changed.
/// `value` is mutated in place.
pub fn segmented_toggle(
    ui: &mut Ui,
    id_source: impl std::hash::Hash,
    value: &mut bool,
    labels: (&str, &str),
) -> bool {
    segmented_select(ui, id_source, &mut (*value as usize), &[labels.0, labels.1])
        .map(|new_idx| {
            let new_val = new_idx == 1;
            let changed = new_val != *value;
            *value = new_val;
            changed
        })
        .unwrap_or(false)
}

/// An N-way segmented selector. Returns `Some(new_index)` only when changed.
pub fn segmented_select(
    ui: &mut Ui,
    _id_source: impl std::hash::Hash,
    selected: &mut usize,
    options: &[&str],
) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let height = 28.0;
    let total_w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, height), Sense::hover());
    let seg_w = total_w / options.len() as f32;
    let painter = ui.painter();

    // Outer border
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, HAIR_2),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(rect, 0.0, Color32::from_rgba_premultiplied(2, 3, 4, 8));

    let mut changed: Option<usize> = None;
    for (i, label) in options.iter().enumerate() {
        let seg_rect = Rect::from_min_size(
            rect.left_top() + Vec2::new(seg_w * i as f32, 0.0),
            Vec2::new(seg_w, height),
        );
        let id = ui.make_persistent_id(("seg", _id_source_hash(&_id_source), i));
        let resp = ui.interact(seg_rect, id, Sense::click());

        let active = *selected == i;
        let hovered = resp.hovered();

        if active {
            painter.rect_filled(seg_rect, 0.0, AMBER);
        } else if hovered {
            painter.rect_filled(
                seg_rect,
                0.0,
                Color32::from_rgba_premultiplied(8, 12, 16, 24),
            );
        }
        if i > 0 {
            painter.line_segment(
                [seg_rect.left_top(), seg_rect.left_bottom()],
                Stroke::new(1.0, HAIR_2),
            );
        }

        let color = if active {
            Color32::from_rgb(0x0a, 0x0a, 0x0a)
        } else {
            INK_3
        };
        let galley = painter.layout_no_wrap(label.to_uppercase(), FontId::monospace(12.0), color);
        let pos = seg_rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, color);

        if resp.clicked() && !active {
            *selected = i;
            changed = Some(i);
        }
    }
    changed
}

// id-source hash helper — egui's `make_persistent_id` is enough but we want
// a stable seed for the interact() ids across frames.
fn _id_source_hash<H: std::hash::Hash>(h: &H) -> u64 {
    use std::hash::BuildHasher;

    std::collections::hash_map::RandomState::new().hash_one(h)
}

// ─────────────────────────────────────────────────────────────────────────────
// Status pill — ● ONLINE / ● OFFLINE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PillState {
    Online,
    Offline,
    Warn,
    Neutral,
}

/// A status pill: filled dot + uppercase label. Online dot pulses subtly.
pub fn status_pill(ui: &mut Ui, label: &str, state: PillState) -> Response {
    let (fg, dot) = match state {
        PillState::Online => (SIGNAL, SIGNAL),
        PillState::Offline => (ALERT, ALERT),
        PillState::Warn => (AMBER, AMBER),
        PillState::Neutral => (INK_3, INK_4),
    };

    let text = label.to_uppercase();
    let font = FontId::monospace(11.0);
    let galley = ui.painter().layout_no_wrap(text, font, fg);
    let pad = Vec2::new(8.0, 4.0);
    let dot_r = 4.0;
    let size = Vec2::new(
        galley.size().x + dot_r * 2.0 + 8.0 + pad.x * 2.0,
        galley.size().y + pad.y * 2.0,
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());

    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, HAIR_2),
        egui::StrokeKind::Inside,
    );

    // pulsing dot
    let t = ui.ctx().input(|i| i.time);
    let pulse = if matches!(state, PillState::Online) {
        0.6 + 0.4 * ((t * 3.5).sin() * 0.5 + 0.5) as f32
    } else {
        1.0
    };
    let dot_center = rect.left_center() + Vec2::new(pad.x + dot_r, 0.0);
    painter.circle_filled(
        dot_center,
        dot_r,
        Color32::from_rgba_premultiplied(
            (dot.r() as f32 * pulse) as u8,
            (dot.g() as f32 * pulse) as u8,
            (dot.b() as f32 * pulse) as u8,
            255,
        ),
    );

    painter.galley(
        rect.left_top() + Vec2::new(pad.x + dot_r * 2.0 + 8.0, pad.y),
        galley,
        fg,
    );

    if matches!(state, PillState::Online) {
        ui.ctx().request_repaint();
    }
    resp
}

// ─────────────────────────────────────────────────────────────────────────────
// Parameter card — label + ID tag + big amber readout + slider + scale
// ─────────────────────────────────────────────────────────────────────────────

/// A full HUD-style parameter row. Use for sliders you want to highlight
/// (Color, Audio, LFO depth etc.).
///
/// Returns `(changed, reset)` — `changed` when the slider moved, `reset` when
/// the card was double-clicked (caller should restore the parameter to its default).
///
/// Layout:
///   NAME                              42.7
///   PARAM/ID                         0…100
///   ──•─────────────────────  (slider w/ ticks)
///   MIN     MID      MAX
pub fn parameter_card_f32(
    ui: &mut Ui,
    name: &str,
    id_tag: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    unit: &str,
) -> (bool, bool) {
    let (min, max) = (*range.start(), *range.end());
    let mut changed = false;

    let frame = egui::Frame::NONE
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, HAIR))
        .inner_margin(egui::Margin::symmetric(12, 10));

    let frame_resp = frame.show(ui, |ui| {
        // Left accent bar — amber when "active" (value != min)
        let accent_color = if *value != min { AMBER } else { INK_4 };
        let panel_rect = ui.max_rect();
        ui.painter().rect_filled(
            Rect::from_min_size(
                panel_rect.left_top() + Vec2::new(-1.0, 8.0),
                Vec2::new(2.0, panel_rect.height() - 16.0),
            ),
            0.0,
            accent_color,
        );

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(name).color(INK).size(13.0));
                ui.label(
                    egui::RichText::new(id_tag.to_uppercase())
                        .color(INK_4)
                        .size(10.0)
                        .monospace(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.vertical(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{}{}", format_value(*value), unit))
                                .color(AMBER)
                                .size(20.0)
                                .strong()
                                .monospace(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}…{}",
                                format_bound(min),
                                format_bound(max)
                            ))
                            .color(INK_4)
                            .size(10.0)
                            .monospace(),
                        );
                    });
                });
            });
        });

        ui.add_space(8.0);

        // The slider itself
        log::trace!("parameter_card_f32 slider value: {}", *value);
        let slider_resp = ui.add(
            egui::Slider::new(value, range.clone())
                .show_value(false)
                .trailing_fill(true),
        );
        if slider_resp.changed() {
            changed = true;
        }

        // Tick row under the slider
        let tick_rect = ui
            .allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover())
            .0;
        let painter = ui.painter();
        let tick_count = 20;
        for i in 0..=tick_count {
            let x = tick_rect.left() + (tick_rect.width() * i as f32 / tick_count as f32);
            let major = i % 5 == 0;
            let h = if major { 4.0 } else { 2.0 };
            painter.line_segment(
                [
                    Pos2::new(x, tick_rect.top()),
                    Pos2::new(x, tick_rect.top() + h),
                ],
                Stroke::new(1.0, if major { HAIR_3 } else { HAIR_2 }),
            );
        }

        // Min / mid / max scale labels
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format_bound(min))
                    .color(INK_4)
                    .size(10.0)
                    .monospace(),
            );
            ui.with_layout(
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.label(
                        egui::RichText::new(format_bound((min + max) / 2.0))
                            .color(INK_4)
                            .size(10.0)
                            .monospace(),
                    );
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.label(
                    egui::RichText::new(format_bound(max))
                        .color(INK_4)
                        .size(10.0)
                        .monospace(),
                );
            });
        });
    });

    let reset = frame_resp.response.double_clicked();
    (changed, reset)
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatting helpers (match the web's number formatting)
// ─────────────────────────────────────────────────────────────────────────────

pub fn format_value(v: f32) -> String {
    let a = v.abs();
    if a >= 100.0 {
        format!("{:.0}", v)
    } else if a >= 10.0 {
        format!("{:.1}", v)
    } else {
        format!("{:.2}", v)
    }
}

pub fn format_bound(v: f32) -> String {
    if v.fract().abs() < 1e-6 {
        format!("{:.0}", v)
    } else if v.abs() >= 10.0 {
        format!("{:.1}", v)
    } else {
        format!("{:.2}", v)
    }
}
