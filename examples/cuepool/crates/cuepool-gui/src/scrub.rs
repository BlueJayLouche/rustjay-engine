//! Shared drag-to-seek behavior for active cue timelines.

use std::time::Duration;

const AUDIO_SEEK_INTERVAL: Duration = Duration::from_millis(67);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeekKind {
    Sound,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeekPhase {
    Drag,
    Release,
}

/// Decide whether the latest coalesced drag target should reach the engine.
pub(crate) fn seek_target_to_emit(
    kind: SeekKind,
    phase: SeekPhase,
    elapsed_since_emit: Option<Duration>,
    pending_target: Option<f32>,
) -> Option<f32> {
    let target = pending_target?;
    if phase == SeekPhase::Release
        || kind == SeekKind::Sound
            && elapsed_since_emit.is_none_or(|elapsed| elapsed >= AUDIO_SEEK_INTERVAL)
    {
        Some(target)
    } else {
        None
    }
}

#[derive(Clone, Copy, Default)]
struct DragState {
    active: bool,
    target: f32,
    pending_target: Option<f32>,
    last_emit_at: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DragUpdate {
    pub(crate) preview_target: Option<f32>,
    pub(crate) emit_target: Option<f32>,
}

/// Track one primary-button scrub in egui's temporary widget memory.
pub(crate) fn update_drag(
    ui: &egui::Ui,
    id: egui::Id,
    response: &egui::Response,
    pointer_target: Option<f32>,
    kind: SeekKind,
) -> DragUpdate {
    let now = ui.input(|input| input.time);
    let primary_down = ui.input(|input| input.pointer.primary_down());
    let primary_released = ui.input(|input| input.pointer.primary_released());
    let mut drag = ui.data_mut(|data| data.get_temp::<DragState>(id).unwrap_or_default());
    let mut emit_target = None;

    if primary_down && response.is_pointer_button_down_on() {
        if let Some(target) = pointer_target
            && (!drag.active || target != drag.target)
        {
            drag.target = target;
            drag.pending_target = Some(target);
        }
        drag.active = true;

        let elapsed = drag
            .last_emit_at
            .map(|last| Duration::from_secs_f64((now - last).max(0.0)));
        emit_target = seek_target_to_emit(kind, SeekPhase::Drag, elapsed, drag.pending_target);
        if emit_target.is_some() {
            drag.pending_target = None;
            drag.last_emit_at = Some(now);
        }
        ui.ctx().request_repaint();
    }

    if drag.active && primary_released {
        if let Some(target) = pointer_target {
            drag.target = target;
        }
        emit_target = seek_target_to_emit(
            kind,
            SeekPhase::Release,
            drag.last_emit_at
                .map(|last| Duration::from_secs_f64((now - last).max(0.0))),
            Some(drag.target),
        );
        ui.data_mut(|data| data.remove::<DragState>(id));
        return DragUpdate {
            preview_target: None,
            emit_target,
        };
    }

    ui.data_mut(|data| data.insert_temp(id, drag));
    DragUpdate {
        preview_target: drag.active.then_some(drag.target),
        emit_target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_drag_emits_at_the_cap() {
        assert_eq!(
            seek_target_to_emit(SeekKind::Sound, SeekPhase::Drag, None, Some(1.0)),
            Some(1.0)
        );
        assert_eq!(
            seek_target_to_emit(
                SeekKind::Sound,
                SeekPhase::Drag,
                Some(Duration::from_millis(66)),
                Some(2.0),
            ),
            None
        );
        assert_eq!(
            seek_target_to_emit(
                SeekKind::Sound,
                SeekPhase::Drag,
                Some(Duration::from_millis(67)),
                Some(3.0),
            ),
            Some(3.0)
        );
    }

    #[test]
    fn release_always_emits_the_final_position() {
        for kind in [SeekKind::Sound, SeekKind::Video] {
            assert_eq!(
                seek_target_to_emit(kind, SeekPhase::Release, Some(Duration::ZERO), Some(4.5),),
                Some(4.5)
            );
        }
    }

    #[test]
    fn video_never_emits_mid_drag() {
        assert_eq!(
            seek_target_to_emit(SeekKind::Video, SeekPhase::Drag, None, Some(6.0)),
            None
        );
        assert_eq!(
            seek_target_to_emit(
                SeekKind::Video,
                SeekPhase::Drag,
                Some(Duration::from_secs(1)),
                Some(7.0),
            ),
            None
        );
    }
}
