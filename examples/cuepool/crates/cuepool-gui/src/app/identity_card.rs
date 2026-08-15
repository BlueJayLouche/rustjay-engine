//! The identity card: CuePool's launch splash and its About box are the same
//! drawing, shown two ways. `Launch` runs on a timer at startup and dismisses
//! itself; `Invoked` is what Help → About opens, with the credits attached.

use std::time::Duration;

/// How long the launch presentation stays up before it has faded out entirely.
pub(crate) const LAUNCH_HOLD: Duration = Duration::from_millis(2400);
/// The tail of [`LAUNCH_HOLD`] spent fading, so the card doesn't just vanish.
pub(crate) const LAUNCH_FADE: Duration = Duration::from_millis(220);
/// Repaint cadence while the card is up, so the torus keeps turning.
pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(33);

const TORUS_WIDTH: usize = 64;
const TORUS_HEIGHT: usize = 22;
const TORUS_RAMP: &[u8] = b".,-~:;=!*#$@";

/// The torus accent, picked per release series so a build announces itself.
const TORUS_COLOURS: [egui::Color32; 7] = [
    egui::Color32::from_rgb(76, 255, 225),
    egui::Color32::from_rgb(255, 124, 159),
    egui::Color32::from_rgb(255, 196, 92),
    egui::Color32::from_rgb(92, 210, 255),
    egui::Color32::from_rgb(154, 255, 150),
    egui::Color32::from_rgb(92, 168, 255),
    egui::Color32::from_rgb(183, 135, 255),
];
const TAGLINE_AMBER: egui::Color32 = egui::Color32::from_rgb(255, 184, 92);

/// Which of the card's two presentations is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardPresentation {
    /// Shown at startup: timed, fades out, and any input ends it early.
    Launch,
    /// Opened from Help → About: no timer, credits attached, closed by the
    /// button, Escape, or a click outside.
    Invoked,
}

impl CardPresentation {
    /// Launch blacks the workspace out; About only dims it, so the card reads
    /// as a layer over the show rather than a screen that replaced it.
    pub(crate) fn backdrop(self) -> egui::Color32 {
        match self {
            Self::Launch => egui::Color32::from_black_alpha(248),
            Self::Invoked => egui::Color32::from_black_alpha(232),
        }
    }
}

/// Draws the card. Returns whether its Close button was pressed, which only
/// the `Invoked` presentation has.
pub(crate) fn identity_card(
    ui: &mut egui::Ui,
    elapsed: f32,
    presentation: CardPresentation,
) -> bool {
    let mut close_requested = false;

    ui.set_width(680.0);
    ui.vertical_centered(|ui| {
        let donut = ui.add(
            egui::Label::new(
                egui::RichText::new(torus_frame(elapsed))
                    .monospace()
                    .size(11.0)
                    .color(torus_colour(
                        env!("CARGO_PKG_VERSION_MAJOR"),
                        env!("CARGO_PKG_VERSION_MINOR"),
                    )),
            )
            .extend()
            .selectable(false),
        );
        donut.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                ui.is_enabled(),
                "Animated CuePool donut",
            )
        });
        ui.add_space(8.0);
        ui.heading(egui::RichText::new("CUEPOOL").size(36.0).strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("AUDIO  /  VIDEO  /  LIGHTING  /  CONTROL")
                .monospace()
                .strong()
                .color(TAGLINE_AMBER),
        );
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(crate::build_identity())
                .monospace()
                .color(egui::Color32::from_gray(180)),
        );

        if presentation == CardPresentation::Invoked {
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label("A professional audio/video playback application");
            ui.hyperlink_to("GitHub", "https://github.com/BlueJayLouche/CuePool");
            ui.label("License: GPL-3.0");
            ui.add_space(12.0);
            close_requested = ui
                .add_sized([104.0, 32.0], egui::Button::new("Close"))
                .clicked();
        }
    });

    close_requested
}

/// Opacity for the launch presentation, given how much of [`LAUNCH_HOLD`] is
/// left. Full brightness until the last [`LAUNCH_FADE`], then quadratic out.
pub(crate) fn launch_opacity(remaining: Duration) -> f32 {
    let remaining = (remaining.as_secs_f32() / LAUNCH_FADE.as_secs_f32()).min(1.0);
    remaining * remaining
}

fn torus_colour(major: &str, minor: &str) -> egui::Color32 {
    let major = major
        .parse::<usize>()
        .expect("Cargo major version is numeric");
    let minor = minor
        .parse::<usize>()
        .expect("Cargo minor version is numeric");
    TORUS_COLOURS[(major + minor) % TORUS_COLOURS.len()]
}

fn torus_frame(elapsed: f32) -> String {
    let mut pixels = vec![b' '; TORUS_WIDTH * TORUS_HEIGHT];
    let mut depth = vec![0.0; pixels.len()];
    let rotation_x = 0.8 + elapsed * 1.25;
    let rotation_z = 0.2 + elapsed * 0.7;
    let (sin_x, cos_x) = rotation_x.sin_cos();
    let (sin_z, cos_z) = rotation_z.sin_cos();
    let mut tube_angle = 0.0_f32;

    while tube_angle < std::f32::consts::TAU {
        let (sin_tube, cos_tube) = tube_angle.sin_cos();
        let mut ring_angle = 0.0_f32;
        while ring_angle < std::f32::consts::TAU {
            let (sin_ring, cos_ring) = ring_angle.sin_cos();
            let ring_radius = 2.0 + cos_tube;
            let x = ring_radius * cos_ring;
            let y = ring_radius * sin_ring;
            let z = sin_tube;
            let rotated_y = y * cos_x - z * sin_x;
            let rotated_z = y * sin_x + z * cos_x;
            let rotated_x = x * cos_z - rotated_y * sin_z;
            let rotated_y = x * sin_z + rotated_y * cos_z;
            let inverse_z = 1.0 / (5.0 + rotated_z);
            let screen_x = (TORUS_WIDTH as f32 / 2.0 + 28.0 * inverse_z * rotated_x) as isize;
            let screen_y = (TORUS_HEIGHT as f32 / 2.0 - 14.0 * inverse_z * rotated_y) as isize;

            let normal_x = cos_tube * cos_ring;
            let normal_y = cos_tube * sin_ring;
            let normal_z = sin_tube;
            let rotated_normal_y = normal_y * cos_x - normal_z * sin_x;
            let rotated_normal_z = normal_y * sin_x + normal_z * cos_x;
            let rotated_normal_y = normal_x * sin_z + rotated_normal_y * cos_z;
            let luminance = rotated_normal_y - rotated_normal_z;

            if luminance > 0.0
                && (0..TORUS_WIDTH as isize).contains(&screen_x)
                && (0..TORUS_HEIGHT as isize).contains(&screen_y)
            {
                let index = screen_y as usize * TORUS_WIDTH + screen_x as usize;
                if inverse_z > depth[index] {
                    depth[index] = inverse_z;
                    let shade = ((luminance * 8.0) as usize).min(TORUS_RAMP.len() - 1);
                    pixels[index] = TORUS_RAMP[shade];
                }
            }

            ring_angle += 0.07;
        }
        tube_angle += 0.15;
    }

    let mut frame = String::with_capacity(pixels.len() + TORUS_HEIGHT - 1);
    for (row, line) in pixels.chunks_exact(TORUS_WIDTH).enumerate() {
        if row > 0 {
            frame.push('\n');
        }
        for &pixel in line {
            frame.push(char::from(pixel));
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
    fn ascii_torus_is_fixed_size_and_animated() {
        let first = torus_frame(0.0);
        let next = torus_frame(0.4);

        assert_ne!(first, next);
        assert_eq!(first.lines().count(), TORUS_HEIGHT);
        assert!(first.lines().all(|line| line.len() == TORUS_WIDTH));
        assert!(
            first
                .bytes()
                .all(|byte| byte == b'\n' || byte == b' ' || TORUS_RAMP.contains(&byte))
        );
    }

    #[test]
    fn torus_colour_changes_with_the_release_series() {
        let v0_5 = torus_colour("0", "5");
        let v0_6 = torus_colour("0", "6");
        let v1_0 = torus_colour("1", "0");

        assert_eq!(v0_5, egui::Color32::from_rgb(92, 168, 255));
        assert_eq!(v0_6, egui::Color32::from_rgb(183, 135, 255));
        assert_ne!(v0_5, v0_6);
        assert_ne!(v0_6, v1_0);
    }

    #[test]
    fn about_dims_the_workspace_less_than_launch() {
        assert!(CardPresentation::Invoked.backdrop().a() < CardPresentation::Launch.backdrop().a());
    }
}
