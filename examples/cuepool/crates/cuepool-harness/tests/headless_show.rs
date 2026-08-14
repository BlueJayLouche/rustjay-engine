mod support;

use cuepool::EngineTrace;
use cuepool_core::{LoopMode, TriggerMode};
use cuepool_harness::{HeadlessShowRunner, RunnerTrace};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::fs;
use support::{Fixture, dummy, sound, video};

fn started(trace: &[RunnerTrace]) -> Vec<i64> {
    trace
        .iter()
        .filter_map(|entry| match entry {
            RunnerTrace::Engine(EngineTrace::CueStarted { qid, .. }) => qid.to_i64(),
            _ => None,
        })
        .collect()
}

fn video_pts(trace: &[RunnerTrace]) -> Vec<f64> {
    trace
        .iter()
        .filter_map(|entry| match entry {
            RunnerTrace::VideoFrame { pts, .. } => Some(*pts),
            _ => None,
        })
        .collect()
}

#[test]
fn loads_relative_media_and_runs_with_last_and_after_last() {
    let fixture = Fixture::new(vec![
        sound(1, TriggerMode::Go),
        dummy(2, TriggerMode::WithLast),
        sound(3, TriggerMode::WithLast),
        dummy(4, TriggerMode::AfterLast),
        dummy(5, TriggerMode::AfterLast),
        dummy(6, TriggerMode::Go),
    ])
    .unwrap();
    let mut runner = HeadlessShowRunner::open(&fixture.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();

    let initial = runner.take_trace();
    assert_eq!(started(&initial), vec![1, 2, 3]);
    assert!(initial.iter().any(|entry| matches!(
        entry,
        RunnerTrace::SideEffect {
            qid: Some(qid),
            kind: "dummy",
        } if *qid == Decimal::TWO
    )));
    assert_eq!(runner.snapshot().standby_qid, Some(Decimal::from(6)));
    assert!(!started(&initial).contains(&4));

    runner.advance_blocks(100).unwrap();
    let trace = runner.take_trace();
    assert_eq!(started(&trace), vec![4, 5]);
    assert!(runner.snapshot().active_cues.is_empty());
}

#[test]
fn missing_and_malformed_projects_report_the_path() {
    let fixture = Fixture::new(Vec::new()).unwrap();
    let missing = fixture.dir().join("missing.qproj");
    let error = HeadlessShowRunner::open(&missing)
        .err()
        .expect("missing project should fail")
        .to_string();
    assert!(error.contains(missing.to_string_lossy().as_ref()));

    let malformed = fixture.dir().join("broken.qproj");
    fs::write(&malformed, "{not json").unwrap();
    let error = HeadlessShowRunner::open(&malformed)
        .err()
        .expect("malformed project should fail")
        .to_string();
    assert!(error.contains(malformed.to_string_lossy().as_ref()));
}

#[test]
fn video_pts_are_not_early_and_overdue_frames_use_newest_due() {
    let fixture = Fixture::new(vec![video(1, LoopMode::OneShot)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&fixture.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.take_trace();

    runner.advance_blocks(3).unwrap();
    assert_eq!(video_pts(&runner.take_trace()), vec![0.0]);
    runner.advance_blocks(1).unwrap();
    assert_eq!(video_pts(&runner.take_trace()), vec![0.04]);

    let mut runner = HeadlessShowRunner::open_with_block_frames(&fixture.project, 4_800).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.take_trace();
    runner.advance_blocks(1).unwrap();
    assert_eq!(video_pts(&runner.take_trace()), vec![0.08]);
}

#[test]
fn video_seek_resets_pending_frames_and_preserves_pause() {
    let fixture = Fixture::new(vec![video(1, LoopMode::OneShot)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&fixture.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.advance_blocks(1).unwrap();
    let instance = runner.snapshot().video.as_ref().unwrap().instance_id;
    runner.take_trace();

    runner.seek(instance, 0.12).unwrap();
    runner.advance_blocks(1).unwrap();
    assert!(
        video_pts(&runner.take_trace())
            .iter()
            .all(|pts| *pts >= 0.12)
    );

    runner.pause().unwrap();
    runner.seek(instance, 0.08).unwrap();
    let seek_trace = runner.take_trace();
    assert!(seek_trace.iter().any(|entry| matches!(
        entry,
        RunnerTrace::VideoSeek {
            target_secs,
            paused: true,
            ..
        } if (*target_secs - 0.08).abs() < 0.0001
    )));
    runner.advance_blocks(5).unwrap();
    assert!(video_pts(&runner.take_trace()).is_empty());
    assert!(runner.snapshot().paused);
    assert!(runner.snapshot().video.as_ref().unwrap().paused);

    runner.resume().unwrap();
    runner.advance_blocks(1).unwrap();
    assert_eq!(video_pts(&runner.take_trace()), vec![0.08]);

    runner.pause().unwrap();
    runner.seek(instance, f32::INFINITY).unwrap();
    let snapshot = runner.snapshot();
    assert!(snapshot.video.as_ref().unwrap().position_secs < 0.2);
    assert!(runner.take_trace().iter().any(|entry| matches!(
        entry,
        RunnerTrace::VideoSeek { target_secs, .. }
            if *target_secs < 0.2 && *target_secs > 0.199
    )));
}

#[test]
fn sound_seek_clamps_and_stays_paused() {
    let fixture = Fixture::new(vec![sound(1, TriggerMode::Go)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&fixture.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.advance_blocks(2).unwrap();
    let instance = runner.snapshot().active_cues[0].instance_id;

    runner.seek(instance, 0.05).unwrap();
    let position = runner.snapshot().active_cues[0].position_secs;
    assert!((position - 0.05).abs() < 0.0001);

    runner.pause().unwrap();
    runner.seek(instance, f32::INFINITY).unwrap();
    let paused = runner.snapshot();
    let position = paused.active_cues[0].position_secs;
    assert!(paused.paused);
    assert!(position < 0.1 && position > 0.099);
    runner.advance_blocks(10).unwrap();
    assert_eq!(runner.snapshot().active_cues[0].position_secs, position);
}

#[test]
fn video_eof_modes_stop_hold_and_reopen_without_stale_epochs() {
    let one_shot = Fixture::new(vec![video(1, LoopMode::OneShot)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&one_shot.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.advance_blocks(20).unwrap();
    assert!(runner.snapshot().video.is_none());

    let hold = Fixture::new(vec![video(1, LoopMode::HoldLast)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&hold.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.advance_blocks(20).unwrap();
    assert!(runner.snapshot().video.is_some());
    assert_eq!(video_pts(&runner.take_trace()).last().copied(), Some(0.16));

    let looping = Fixture::new(vec![video(1, LoopMode::LoopedInfinite)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&looping.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.take_trace();
    runner.advance_blocks(22).unwrap();
    let trace = runner.take_trace();
    let eof_epochs: Vec<u64> = trace
        .iter()
        .filter_map(|entry| match entry {
            RunnerTrace::VideoEof { epoch, .. } => Some(*epoch),
            _ => None,
        })
        .collect();
    assert_eq!(eof_epochs, vec![1]);
    assert_eq!(runner.snapshot().video.as_ref().unwrap().epoch, 2);
    assert!(
        trace
            .iter()
            .any(|entry| matches!(entry, RunnerTrace::VideoFrame { epoch: 2, .. }))
    );
}

#[test]
fn stop_and_project_replacement_clear_runtime_state() {
    let mut delayed = sound(2, TriggerMode::WithLast);
    delayed.base_mut().delay = cuepool_core::Timespan::from_secs_f64(1.0);
    let fixture = Fixture::new(vec![sound(1, TriggerMode::Go), delayed]).unwrap();
    let replacement = Fixture::new(vec![dummy(9, TriggerMode::Go)]).unwrap();
    let mut runner = HeadlessShowRunner::open(&fixture.project).unwrap();
    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    assert!(!runner.snapshot().active_cues.is_empty());

    runner.stop().unwrap();
    let snapshot = runner.snapshot();
    assert!(snapshot.active_cues.is_empty());
    assert!(snapshot.video.is_none());
    assert!(!snapshot.paused);

    runner.select(Decimal::ONE).unwrap();
    runner.go().unwrap();
    runner.take_trace();
    runner.replace_project(&replacement.project).unwrap();
    runner.advance_blocks(120).unwrap();
    assert!(!started(&runner.take_trace()).contains(&2));
    assert!(runner.snapshot().active_cues.is_empty());
    assert_eq!(runner.snapshot().standby_qid, None);
}
