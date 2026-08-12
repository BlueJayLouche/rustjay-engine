//! Take Editor — per-channel curve editing for `.dmxrec` recordings.
//!
//! Scope (see `DMX_RECORDER.md`): view curves + scrub, drag/insert/delete
//! points, flatten a time range to a constant, trim the take, delete a
//! channel, per-channel time-shift. Everything operates on an in-memory
//! working copy; Save rotates the on-disk take to `.prev` (same single-level
//! revert convention as the recorder) before writing.
//!
//! The editor is GUI-local: it loads/saves via `rustjay-lighting` directly.
//! "Output scrub" pushes the frame at the playhead to the lighting output
//! through [`AppCommand::RecorderScrub`].

use rustjay_lighting::{rec_duration_ms, MaskedFrame, RecEvent, ShowPlayer};

use crate::app::{AppCommand, SharedStateHandle};

/// One editor-level undo step (cloned before each destructive op).
type Snapshot = Vec<RecEvent>;

#[derive(Default)]
pub struct TakeEditor {
    pub open: bool,
    path: String,
    /// Working copy, kept sorted by time.
    events: Vec<RecEvent>,
    /// Distinct (universe, channel) pairs, sorted.
    channels: Vec<(u16, u16)>,
    sel: Option<(u16, u16)>,
    dirty: bool,
    error: Option<String>,
    /// Playhead position.
    scrub_ms: u32,
    output_scrub: bool,
    /// Horizontal zoom (1.0 = whole take fits) and scroll (0..1 of hidden width).
    zoom: f32,
    scroll: f32,
    /// Single-level undo.
    undo: Option<Snapshot>,
    /// Index into `events` of the point being dragged.
    drag_idx: Option<usize>,
    /// Cached playback state for scrub output; rebuilt after any edit.
    player: Option<ShowPlayer>,
    // Operation parameters (seconds / value).
    range: (f32, f32),
    flat_value: u8,
    shift_ms: i32,
}

impl TakeEditor {
    /// Load `path` into the editor and open the window.
    pub fn open_file(&mut self, path: String) {
        match rustjay_lighting::read_rec(&path) {
            Ok(events) => {
                self.events = events;
                self.error = None;
            }
            Err(e) => {
                self.events = Vec::new();
                self.error = Some(format!("cannot read '{path}': {e}"));
            }
        }
        self.path = path;
        self.dirty = false;
        self.undo = None;
        self.drag_idx = None;
        self.player = None;
        self.scrub_ms = 0;
        self.zoom = 1.0;
        self.scroll = 0.0;
        self.rebuild_channels();
        self.range = (0.0, self.duration_ms() as f32 / 1000.0);
        self.open = true;
    }

    fn duration_ms(&self) -> u32 {
        rec_duration_ms(&self.events)
    }

    fn rebuild_channels(&mut self) {
        self.channels = self.events.iter().map(|e| (e.universe, e.channel)).collect();
        self.channels.sort_unstable();
        self.channels.dedup();
        if self.sel.is_none_or(|s| !self.channels.contains(&s)) {
            self.sel = self.channels.first().copied();
        }
    }

    fn snapshot_undo(&mut self) {
        self.undo = Some(self.events.clone());
    }

    /// Mark the working copy edited: sort, refresh caches.
    fn edited(&mut self) {
        self.events.sort_by_key(|e| e.t_ms);
        self.dirty = true;
        self.player = None;
        self.rebuild_channels();
    }

    /// Channel state at `t_ms` (whole take), for trim/flatten resume values.
    fn state_at(&mut self, t_ms: u32) -> MaskedFrame {
        let player = self
            .player
            .get_or_insert_with(|| ShowPlayer::new(self.events.clone()));
        player.seek(t_ms);
        player.frame().clone()
    }

    fn save(&mut self) {
        let path = std::path::PathBuf::from(&self.path);
        let result = (|| -> std::io::Result<()> {
            if path.exists() {
                let mut prev = path.as_os_str().to_owned();
                prev.push(".prev");
                std::fs::rename(&path, std::path::PathBuf::from(prev))?;
            }
            let mut w = rustjay_lighting::RecWriter::create(&path)?;
            for e in &self.events {
                w.write(*e)?;
            }
            w.finish()
        })();
        match result {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("save failed: {e}")),
        }
    }

    /// Render the editor window; call every frame.
    pub fn show(&mut self, ctx: &egui::Context, state: &SharedStateHandle) {
        if !self.open {
            return;
        }
        let recording = state
            .lock()
            .map(|s| s.recorder_status.recording)
            .unwrap_or(false);

        let mut open = self.open;
        let mut scrub_cmd: Option<AppCommand> = None;
        egui::Window::new("Take Editor")
            .collapsible(false)
            .resizable(true)
            .default_size([760.0, 420.0])
            .open(&mut open)
            .show(ctx, |ui| {
                self.header_ui(ui, recording);
                ui.add_enabled_ui(!recording, |ui| {
                    self.channel_ops_ui(ui);
                    self.range_ops_ui(ui);
                });
                self.curve_ui(ui, !recording, &mut scrub_cmd);
                if let Some(err) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
                if recording {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Recording in progress — editing disabled.",
                    );
                }
            });
        // Window closed this frame → release any scrub output.
        if self.open && !open && self.output_scrub {
            scrub_cmd = Some(AppCommand::RecorderScrub { frame: None });
        }
        self.open = open;
        if let Some(cmd) = scrub_cmd
            && let Ok(mut s) = state.lock() {
                s.command_queue.push(cmd);
            }
    }

    fn header_ui(&mut self, ui: &mut egui::Ui, recording: bool) {
        ui.horizontal(|ui| {
            let name = std::path::Path::new(&self.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| self.path.clone());
            ui.label(egui::RichText::new(name).monospace());
            if self.dirty {
                ui.label(egui::RichText::new("(edited)").weak());
            }
            ui.label(
                egui::RichText::new(format!(
                    "{:.1}s · {} events · {} channels",
                    self.duration_ms() as f32 / 1000.0,
                    self.events.len(),
                    self.channels.len()
                ))
                .small()
                .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.dirty && !recording, egui::Button::new("💾 Save"))
                    .on_hover_text("Write the take (previous version kept as .prev)")
                    .clicked()
                {
                    self.save();
                }
                if ui
                    .add_enabled(!recording, egui::Button::new("Reload"))
                    .on_hover_text("Discard edits and re-read the take from disk")
                    .clicked()
                {
                    let path = self.path.clone();
                    self.open_file(path);
                }
                if ui
                    .add_enabled(self.undo.is_some() && !recording, egui::Button::new("Undo"))
                    .on_hover_text("Undo the last edit (single level)")
                    .clicked()
                    && let Some(prev) = self.undo.take() {
                        self.events = prev;
                        self.edited();
                    }
            });
        });
    }

    fn channel_ops_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Channel:");
            let sel_label = self
                .sel
                .map(|(u, c)| format!("U{u} ch{}", c + 1))
                .unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("take_ed_channel")
                .selected_text(sel_label)
                .show_ui(ui, |ui| {
                    for &(u, c) in &self.channels {
                        ui.selectable_value(&mut self.sel, Some((u, c)), format!("U{u} ch{}", c + 1));
                    }
                });
            let has_sel = self.sel.is_some();
            if ui
                .add_enabled(has_sel, egui::Button::new("Delete channel"))
                .on_hover_text("Remove all of this channel's events — other sources own it again")
                .clicked()
                && let Some((u, c)) = self.sel {
                    self.snapshot_undo();
                    self.events.retain(|e| (e.universe, e.channel) != (u, c));
                    self.edited();
                }
            ui.separator();
            ui.label("Shift (ms):");
            ui.add(egui::DragValue::new(&mut self.shift_ms).speed(10).range(-3_600_000..=3_600_000));
            if ui
                .add_enabled(has_sel && self.shift_ms != 0, egui::Button::new("Apply"))
                .on_hover_text("Time-shift every event of this channel (clamped at 0)")
                .clicked()
                && let Some((u, c)) = self.sel {
                    self.snapshot_undo();
                    for e in &mut self.events {
                        if (e.universe, e.channel) == (u, c) {
                            e.t_ms = e.t_ms.saturating_add_signed(self.shift_ms);
                        }
                    }
                    self.edited();
                }
        });
    }

    fn range_ops_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Range (s):");
            ui.add(egui::DragValue::new(&mut self.range.0).speed(0.1).range(0.0..=86_400.0));
            ui.label("–");
            ui.add(egui::DragValue::new(&mut self.range.1).speed(0.1).range(0.0..=86_400.0));
            let valid = self.range.1 > self.range.0;
            if ui
                .add_enabled(valid, egui::Button::new("Trim take"))
                .on_hover_text("Keep only this window; held values at the start become the new baseline")
                .clicked()
            {
                self.trim();
            }
            ui.separator();
            ui.label("Flatten to:");
            ui.add(egui::DragValue::new(&mut self.flat_value).speed(1).range(0..=255));
            if ui
                .add_enabled(valid && self.sel.is_some(), egui::Button::new("Flatten"))
                .on_hover_text("Hold this channel at a constant over the range; the old curve resumes after")
                .clicked()
            {
                self.flatten();
            }
        });
    }

    /// Trim the take to `range`: state held at range-start becomes t=0
    /// baseline events, in-range events shift left, everything else drops.
    fn trim(&mut self) {
        let start = (self.range.0 * 1000.0) as u32;
        let end = (self.range.1 * 1000.0) as u32;
        self.snapshot_undo();
        let baseline = self.state_at(start);
        let mut out: Vec<RecEvent> = Vec::new();
        for (universe, masked) in baseline.iter() {
            for ch in 0..rustjay_lighting::DMX_UNIVERSE_SIZE {
                if masked.owned(ch) && masked.values()[ch] != 0 {
                    out.push(RecEvent {
                        t_ms: 0,
                        universe,
                        channel: ch as u16,
                        value: masked.values()[ch],
                    });
                }
            }
        }
        out.extend(
            self.events
                .iter()
                .filter(|e| e.t_ms > start && e.t_ms <= end)
                .map(|e| RecEvent { t_ms: e.t_ms - start, ..*e }),
        );
        self.events = out;
        self.edited();
        self.scrub_ms = 0;
        self.range = (0.0, self.duration_ms() as f32 / 1000.0);
    }

    /// Flatten the selected channel to `flat_value` over `range`; the value
    /// in effect at range-end is re-asserted so the old curve resumes.
    fn flatten(&mut self) {
        let Some((u, c)) = self.sel else { return };
        let start = (self.range.0 * 1000.0) as u32;
        let end = (self.range.1 * 1000.0) as u32;
        self.snapshot_undo();
        let resume = self
            .state_at(end)
            .get(u)
            .map_or(0, |m| m.values()[c as usize]);
        self.events.retain(|e| {
            (e.universe, e.channel) != (u, c) || e.t_ms < start || e.t_ms > end
        });
        self.events.push(RecEvent { t_ms: start, universe: u, channel: c, value: self.flat_value });
        self.events.push(RecEvent { t_ms: end, universe: u, channel: c, value: resume });
        self.edited();
    }

    #[cfg(test)]
    fn from_events(events: Vec<RecEvent>) -> Self {
        let mut ed = Self { events, ..Default::default() };
        ed.rebuild_channels();
        ed
    }

    /// The curve canvas: step curve + draggable points + scrub playhead.
    fn curve_ui(&mut self, ui: &mut egui::Ui, editable: bool, scrub_cmd: &mut Option<AppCommand>) {
        ui.horizontal(|ui| {
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=100.0).logarithmic(true));
            ui.add_enabled(
                self.zoom > 1.0,
                egui::Slider::new(&mut self.scroll, 0.0..=1.0).show_value(false).text("scroll"),
            );
            let changed = ui
                .checkbox(&mut self.output_scrub, "Output scrub")
                .on_hover_text("Send the frame at the playhead to the lighting output")
                .changed();
            if changed && !self.output_scrub {
                *scrub_cmd = Some(AppCommand::RecorderScrub { frame: None });
            }
        });

        let dur = self.duration_ms().max(1) as f32;
        let view_w = dur / self.zoom;
        let t0 = self.scroll * (dur - view_w);
        let t1 = t0 + view_w;

        let height = 220.0;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, egui::Color32::from_gray(24));

        let to_x = |t: f32| rect.left() + (t - t0) / (t1 - t0) * rect.width();
        let to_t = |x: f32| (t0 + (x - rect.left()) / rect.width() * (t1 - t0)).clamp(0.0, dur);
        let to_y = |v: u8| rect.bottom() - v as f32 / 255.0 * rect.height();
        let to_v = |y: f32| (255.0 * (rect.bottom() - y) / rect.height()).clamp(0.0, 255.0) as u8;

        // Value gridlines.
        for v in [0u8, 64, 128, 192, 255] {
            let y = to_y(v);
            painter.hline(rect.x_range(), y, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(38)));
            painter.text(
                egui::pos2(rect.left() + 2.0, y),
                egui::Align2::LEFT_BOTTOM,
                v.to_string(),
                egui::FontId::proportional(9.0),
                egui::Color32::from_gray(90),
            );
        }

        // Step curve + point handles for the selected channel.
        let mut hover_idx: Option<usize> = None;
        if let Some((su, sc)) = self.sel {
            let idxs: Vec<usize> = (0..self.events.len())
                .filter(|&i| (self.events[i].universe, self.events[i].channel) == (su, sc))
                .collect();
            let stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(120, 190, 255));
            let mut last: Option<(f32, u8)> = None;
            for &i in &idxs {
                let e = self.events[i];
                let (t, v) = (e.t_ms as f32, e.value);
                let (pt, pv) = last.unwrap_or((0.0, 0));
                // Hold previous value, then jump at the event time.
                painter.line_segment([egui::pos2(to_x(pt), to_y(pv)), egui::pos2(to_x(t), to_y(pv))], stroke);
                painter.line_segment([egui::pos2(to_x(t), to_y(pv)), egui::pos2(to_x(t), to_y(v))], stroke);
                last = Some((t, v));
            }
            if let Some((t, v)) = last {
                painter.line_segment([egui::pos2(to_x(t), to_y(v)), egui::pos2(rect.right(), to_y(v))], stroke);
            }

            // Point handles + hit test.
            let pointer = resp.hover_pos();
            for &i in &idxs {
                let e = self.events[i];
                let p = egui::pos2(to_x(e.t_ms as f32), to_y(e.value));
                let near = pointer.is_some_and(|m| m.distance(p) < 8.0);
                if near && hover_idx.is_none() {
                    hover_idx = Some(i);
                }
                painter.circle_filled(
                    p,
                    if near || self.drag_idx == Some(i) { 5.0 } else { 3.0 },
                    if near || self.drag_idx == Some(i) {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(120, 190, 255)
                    },
                );
            }

            if editable {
                // Drag a point: time clamped between neighbours, value free.
                if resp.drag_started() && hover_idx.is_some() {
                    self.snapshot_undo();
                    self.drag_idx = hover_idx;
                }
                if let (Some(di), Some(pos)) = (self.drag_idx, resp.interact_pointer_pos()) {
                    let pos_in_list = idxs.iter().position(|&i| i == di);
                    let (lo, hi) = match pos_in_list {
                        Some(k) => (
                            k.checked_sub(1).map(|p| self.events[idxs[p]].t_ms + 1).unwrap_or(0),
                            idxs.get(k + 1).map(|&n| self.events[n].t_ms - 1).unwrap_or(u32::MAX),
                        ),
                        None => (0, u32::MAX),
                    };
                    self.events[di].t_ms = (to_t(pos.x) as u32).clamp(lo, hi.max(lo));
                    self.events[di].value = to_v(pos.y);
                    self.dirty = true;
                    self.player = None;
                }
                if resp.drag_stopped() && self.drag_idx.take().is_some() {
                    self.edited();
                }
                // Double-click empty space: insert a point.
                if resp.double_clicked() && hover_idx.is_none()
                    && let Some(pos) = resp.interact_pointer_pos() {
                        self.snapshot_undo();
                        self.events.push(RecEvent {
                            t_ms: to_t(pos.x) as u32,
                            universe: su,
                            channel: sc,
                            value: to_v(pos.y),
                        });
                        self.edited();
                    }
                // Right-click a point: delete it.
                if resp.secondary_clicked()
                    && let Some(i) = hover_idx {
                        self.snapshot_undo();
                        self.events.remove(i);
                        self.edited();
                    }
                // Plain drag on empty canvas scrubs the playhead.
                if resp.dragged() && self.drag_idx.is_none()
                    && let Some(pos) = resp.interact_pointer_pos() {
                        let t = to_t(pos.x) as u32;
                        if t != self.scrub_ms {
                            self.scrub_ms = t;
                            if self.output_scrub {
                                let frame = self.state_at(t);
                                *scrub_cmd =
                                    Some(AppCommand::RecorderScrub { frame: Some(frame) });
                            }
                        }
                    }
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No channels — record or load a take",
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(110),
            );
        }

        // Playhead.
        let px = to_x(self.scrub_ms as f32);
        if rect.x_range().contains(px) {
            painter.vline(px, rect.y_range(), egui::Stroke::new(1.0_f32, egui::Color32::YELLOW));
        }
        painter.text(
            egui::pos2(px + 3.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{:.2}s", self.scrub_ms as f32 / 1000.0),
            egui::FontId::proportional(10.0),
            egui::Color32::YELLOW,
        );
        ui.label(
            egui::RichText::new(
                "Drag point = move · double-click = insert · right-click = delete · drag empty = scrub",
            )
            .small()
            .weak(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(t_ms: u32, universe: u16, channel: u16, value: u8) -> RecEvent {
        RecEvent { t_ms, universe, channel, value }
    }

    #[test]
    fn trim_materialises_held_values_as_baseline() {
        // ch0 = 100 at t=0 (held through the trim window); ch1 changes inside it.
        let mut ed = TakeEditor::from_events(vec![
            ev(0, 1, 0, 100),
            ev(2000, 1, 1, 50),
            ev(9000, 1, 0, 7), // beyond the window — dropped
        ]);
        ed.range = (1.0, 5.0);
        ed.trim();

        assert_eq!(
            ed.events,
            vec![ev(0, 1, 0, 100), ev(1000, 1, 1, 50)],
            "held ch0 becomes t=0 baseline; in-window events shift left; tail dropped"
        );
        assert!(ed.dirty);
        assert!(ed.undo.is_some(), "trim is undoable");
    }

    #[test]
    fn flatten_holds_range_and_resumes() {
        // ch0: 10 → 200 (t=2s) → 30 (t=4s).
        let mut ed = TakeEditor::from_events(vec![
            ev(0, 1, 0, 10),
            ev(2000, 1, 0, 200),
            ev(4000, 1, 0, 30),
        ]);
        ed.sel = Some((1, 0));
        ed.range = (1.0, 3.0);
        ed.flat_value = 99;
        ed.flatten();

        // t=2s event swallowed; plateau 99 over [1s,3s]; resume value at 3s
        // was 200 (the swallowed curve), re-asserted at the range end.
        assert_eq!(
            ed.events,
            vec![ev(0, 1, 0, 10), ev(1000, 1, 0, 99), ev(3000, 1, 0, 200), ev(4000, 1, 0, 30)]
        );
    }

    #[test]
    fn shift_clamps_at_zero_and_other_channels_untouched() {
        let mut ed = TakeEditor::from_events(vec![
            ev(100, 1, 0, 10),
            ev(500, 1, 0, 20),
            ev(300, 2, 0, 77),
        ]);
        // Shift channel (1,0) by -200ms: first event clamps to 0.
        ed.snapshot_undo();
        for e in &mut ed.events {
            if (e.universe, e.channel) == (1, 0) {
                e.t_ms = e.t_ms.saturating_add_signed(-200);
            }
        }
        ed.edited();
        assert_eq!(
            ed.events,
            vec![ev(0, 1, 0, 10), ev(300, 1, 0, 20), ev(300, 2, 0, 77)]
        );
    }

    #[test]
    fn state_at_reflects_playhead() {
        let mut ed = TakeEditor::from_events(vec![ev(0, 1, 0, 10), ev(2000, 1, 0, 200)]);
        assert_eq!(ed.state_at(1000).get(1).unwrap().values()[0], 10);
        assert_eq!(ed.state_at(2500).get(1).unwrap().values()[0], 200);
        // Backwards seek (scrubbing left) replays correctly.
        assert_eq!(ed.state_at(500).get(1).unwrap().values()[0], 10);
    }
}
