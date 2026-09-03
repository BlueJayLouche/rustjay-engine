//! The splash: KOVVBOJ's launch screen and its About box are the same drawing,
//! shown two ways. `Launch` runs on a timer at startup and fades itself out;
//! `About` is what Help → About opens, with the credits attached.
//!
//! The art is an ASCII plasma field with the wordmark knocked out of it —
//! a video synth introducing itself in the only medium a text label has.

use std::time::Duration;

/// How long the launch screen stays up before it has faded out entirely.
///
/// Long, because most of it is the fade and the plasma is worth watching. Any
/// click or keypress skips the rest, so it is never in the way of a soundcheck.
pub const LAUNCH_HOLD: Duration = Duration::from_millis(7000);
/// The tail of [`LAUNCH_HOLD`] spent fading, so it doesn't just vanish.
pub const LAUNCH_FADE: Duration = Duration::from_millis(4000);
/// Repaint cadence while the splash is up, so the plasma keeps moving.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(33);

const W: usize = 88;
const H: usize = 17;
/// Dark-to-light glyph ramp. The background is drawn from its dim end and the
/// wordmark from its dense end, so the letters read even as both shimmer.
const RAMP: &[u8] = b" .:-=+*#%@";

/// 5x5 cells for K, O, V, V, B, O, J — the only letters this app needs.
const GLYPHS: [[&str; 5]; 7] = [
    ["#   #", "#  # ", "###  ", "#  # ", "#   #"],
    [" ### ", "#   #", "#   #", "#   #", " ### "],
    ["#   #", "#   #", "#   #", " # # ", "  #  "],
    ["#   #", "#   #", "#   #", " # # ", "  #  "],
    ["#### ", "#   #", "#### ", "#   #", "#### "],
    [" ### ", "#   #", "#   #", "#   #", " ### "],
    ["  ###", "    #", "    #", "#   #", " ### "],
];
/// Every glyph column is drawn twice: a monospace cell is about half as wide
/// as it is tall, so a single-cell stroke reads far thinner than a single-row
/// one and the letters dissolve into the field behind them.
const SCALE: usize = 2;
/// Ten drawn columns per letter, then two of gap.
const PITCH: usize = 5 * SCALE + 2;
const WORD_W: usize = GLYPHS.len() * PITCH - 2;
const WORD_X: usize = (W - WORD_W) / 2;
const WORD_Y: usize = 6;

/// Which presentation is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// Shown at startup: timed, and fades out on its own.
    Launch,
    /// Opened from Help → About: no timer, credits, closed by the button,
    /// Escape, or a click outside.
    About,
}

impl Presentation {
    /// Launch blacks the workspace out; About only dims it, so the card reads
    /// as a layer over the show rather than a screen that replaced it.
    pub fn backdrop(self) -> egui::Color32 {
        match self {
            Self::Launch => egui::Color32::from_black_alpha(250),
            Self::About => egui::Color32::from_black_alpha(226),
        }
    }

    /// The card's own panel — which `Launch` does without.
    ///
    /// A `Frame` paints its background from the *outer* `Ui`, which never sees
    /// the `multiply_opacity` the fade sets on the content. A panel here would
    /// therefore hang at full opacity over a backdrop that had already faded
    /// out, and the launch screen would end by shedding its dimming a beat
    /// before the card itself went. The backdrop is the launch screen's
    /// background anyway; About is a dialog, and never fades.
    pub fn frame(self, style: &egui::Style) -> egui::Frame {
        match self {
            Self::Launch => egui::Frame::NONE.inner_margin(24.0),
            Self::About => egui::Frame::popup(style),
        }
    }
}

/// Draws the splash. Returns whether its Close button was pressed, which only
/// the `About` presentation has.
pub fn splash(ui: &mut egui::Ui, elapsed: f32, presentation: Presentation) -> bool {
    use rustjay_gui::egui_theme::colors::*;
    let mut close_requested = false;

    ui.set_width(640.0);
    ui.vertical_centered(|ui| {
        let art = ui.add(
            egui::Label::new(plasma_job(elapsed, amber_dim().gamma_multiply(0.55), amber()))
                .extend()
                .selectable(false),
        );
        art.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), "KOVVBOJ")
        });
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("MIX  /  STAGE  /  MAP")
                .monospace()
                .strong()
                .color(signal()),
        );
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                .monospace()
                .color(ink_3()),
        );

        if presentation == Presentation::About {
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label("A live video synthesis and projection instrument");
            ui.hyperlink_to("GitHub", "https://github.com/BlueJayLouche/rustjay-engine");
            ui.add_space(12.0);
            close_requested = ui
                .add_sized([104.0, 32.0], egui::Button::new("Close"))
                .clicked();
        }
    });

    close_requested
}

/// Opacity for the launch card, given how much of [`LAUNCH_HOLD`] is left.
/// Full brightness until the last [`LAUNCH_FADE`], then quadratic out.
pub fn launch_opacity(remaining: Duration) -> f32 {
    let remaining = (remaining.as_secs_f32() / LAUNCH_FADE.as_secs_f32()).min(1.0);
    remaining * remaining
}

/// Opacity for the backdrop behind it, which has to fade more slowly.
///
/// A black veil reads as gone long before its alpha does — halfway through the
/// fade the workspace is already back at what looks like full brightness —
/// while bright text over it stays legible nearly to zero. Sharing one curve
/// therefore ends the launch screen twice: the room lights come up, and the
/// wordmark hangs there afterwards. So the backdrop gets a much gentler curve,
/// holding the dim until the card has all but gone. Both still reach zero on
/// the same frame.
pub fn backdrop_opacity(remaining: Duration) -> f32 {
    /// Chosen by eye: the workspace stays visibly dim while the wordmark is
    /// still legible, rather than snapping back at the halfway point.
    const HOLD: f32 = 0.35;
    launch_opacity(remaining).powf(HOLD)
}

/// Is this cell part of a letter?
fn in_word(x: usize, y: usize) -> bool {
    if !(WORD_Y..WORD_Y + 5).contains(&y) || !(WORD_X..WORD_X + WORD_W).contains(&x) {
        return false;
    }
    let col = x - WORD_X;
    // The last two columns of each pitch are the gap, and have no glyph cell.
    col % PITCH < 5 * SCALE
        && GLYPHS[col / PITCH][y - WORD_Y].as_bytes()[col % PITCH / SCALE] == b'#'
}

/// Cells near a letter, blanked so the plasma doesn't crowd the wordmark.
/// Wider than tall because the letters are 5 cells apart but only 1 row.
fn near_word(x: usize, y: usize) -> bool {
    (-1..=1).any(|dy: isize| {
        (-3..=3).any(|dx: isize| {
            in_word(
                x.saturating_add_signed(dx),
                y.saturating_add_signed(dy),
            )
        })
    })
}

/// One frame, laid out with the field and the wordmark in separate colours.
///
/// A `RichText` is one colour throughout, and a wordmark drawn in the same
/// colour as the field it sits in reads as more field. So the frame is walked
/// once more and cut into runs wherever it crosses a letter edge.
fn plasma_job(elapsed: f32, field: egui::Color32, word: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::no_max_width(),
        ..Default::default()
    };
    let format = |colour| egui::TextFormat {
        font_id: egui::FontId::monospace(11.0),
        color: colour,
        ..Default::default()
    };
    let frame = plasma_frame(elapsed);
    let mut run = String::new();
    let mut lit = false;
    for (y, line) in frame.lines().enumerate() {
        for (x, ch) in line.chars().enumerate() {
            if in_word(x, y) != lit {
                job.append(&run, 0.0, format(if lit { word } else { field }));
                run.clear();
                lit = !lit;
            }
            run.push(ch);
        }
        run.push('\n');
    }
    job.append(&run, 0.0, format(if lit { word } else { field }));
    job
}

/// One frame of the plasma, with the wordmark punched through it.
fn plasma_frame(elapsed: f32) -> String {
    let mut frame = String::with_capacity((W + 1) * H);
    let top = (RAMP.len() - 1) as f32;
    for y in 0..H {
        if y > 0 {
            frame.push('\n');
        }
        for x in 0..W {
            let fx = x as f32;
            // Monospace cells are about twice as tall as they are wide, so the
            // vertical term is doubled to keep the field from looking squashed.
            let fy = y as f32 * 2.0;
            let field = (fx * 0.18 + elapsed * 1.7).sin()
                + (fy * 0.22 + elapsed * 1.1).sin()
                + (fx * 0.12 + fy * 0.16 + elapsed * 0.9).sin()
                + (((fx - W as f32 / 2.0).powi(2) + (fy - H as f32).powi(2)).sqrt() * 0.13
                    - elapsed * 2.0)
                    .sin();
            let field = ((field + 4.0) / 8.0).clamp(0.0, 1.0);
            frame.push(char::from(if in_word(x, y) {
                RAMP[((0.8 + field * 0.2) * top) as usize]
            } else if near_word(x, y) {
                b' '
            } else {
                RAMP[(field * 0.32 * top) as usize]
            }));
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_fades_only_at_the_end() {
        assert_eq!(launch_opacity(LAUNCH_HOLD), 1.0);
        assert_eq!(launch_opacity(LAUNCH_FADE), 1.0);
        assert_eq!(launch_opacity(LAUNCH_FADE / 2), 0.25);
        assert_eq!(launch_opacity(Duration::ZERO), 0.0);
    }

    #[test]
    fn the_backdrop_outlasts_the_card_it_sits_behind() {
        // Equal at both ends, and the backdrop is ahead everywhere between.
        assert_eq!(backdrop_opacity(LAUNCH_FADE), launch_opacity(LAUNCH_FADE));
        assert_eq!(backdrop_opacity(Duration::ZERO), 0.0);
        for part in [1, 2, 4, 8] {
            let remaining = LAUNCH_FADE / part * (part - 1).max(1);
            assert!(
                backdrop_opacity(remaining) >= launch_opacity(remaining),
                "backdrop fell behind the card with {remaining:?} left"
            );
        }
        assert!(backdrop_opacity(LAUNCH_FADE / 2) > launch_opacity(LAUNCH_FADE / 2));
    }

    #[test]
    fn the_launch_card_has_no_panel_to_outlive_its_fade() {
        let style = egui::Style::default();

        assert_eq!(Presentation::Launch.frame(&style).fill.a(), 0);
        assert!(Presentation::About.frame(&style).fill.a() > 0);
        assert!(Presentation::About.backdrop().a() < Presentation::Launch.backdrop().a());
    }

    #[test]
    fn plasma_is_fixed_size_and_animated() {
        let first = plasma_frame(0.0);
        let next = plasma_frame(0.4);

        assert_ne!(first, next);
        assert_eq!(first.lines().count(), H);
        assert!(first.lines().all(|line| line.len() == W));
        assert!(
            first
                .bytes()
                .all(|b| b == b'\n' || RAMP.contains(&b))
        );
    }

    #[test]
    fn the_wordmark_survives_the_plasma() {
        // Every letter row is drawn from the dense end of the ramp, whatever
        // the field is doing under it, and the gap columns stay blank.
        for t in [0.0, 0.9, 3.7] {
            let frame = plasma_frame(t);
            let rows: Vec<&str> = frame.lines().collect();
            for (y, row) in rows.iter().enumerate().skip(WORD_Y).take(5) {
                for x in WORD_X..WORD_X + WORD_W {
                    let ch = row.as_bytes()[x];
                    if in_word(x, y) {
                        assert!(b"#%@".contains(&ch), "letter cell ({x},{y}) drew {ch:?}");
                    } else {
                        assert_eq!(ch, b' ', "gap cell ({x},{y}) drew {ch:?}");
                    }
                }
            }
        }
    }
}

