//! Deterministic development and visual-testing fixtures for the CuePool UI.

use crate::app::{
    ActiveCueInfo, CuePoolApp, CueState, Diagnostics, GuiMeterData, OutputDiagnostics,
    RecorderStatus, SharedState, VideoDiagnostics,
};
use chrono::NaiveDate;
use cuepool_core::{
    AudioRouting, CanvasFit, Cue, CueBase, FadeType, MonitorId, SerializedColour, ShowFile,
    StopMode, Timespan, TriggerMode,
};
use rust_decimal::Decimal;

fn cue_base(qid: Decimal, name: &str, parent: Option<Decimal>) -> CueBase {
    CueBase {
        qid,
        parent,
        name: name.into(),
        ..Default::default()
    }
}

/// A representative in-memory show with stable content and no media I/O.
pub fn demo_show() -> ShowFile {
    let group = Decimal::ONE;
    let sound = Decimal::new(11, 1);
    let video = Decimal::new(12, 1);
    let text = Decimal::new(13, 1);

    let mut show = ShowFile::default();
    show.show_settings.title = "Museum Gala Preview".into();
    show.show_settings.description = "Deterministic CuePool operator preview".into();
    show.show_settings.author = "CuePool Team".into();
    show.show_settings.date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    show.show_settings.remote_nodes.clear();
    show.cues = vec![
        Cue::Group {
            base: cue_base(group, "Opening Sequence", None),
        },
        Cue::Sound {
            base: cue_base(sound, "Lobby Ambience", Some(group)),
            path: String::new(),
            start_time: Timespan::ZERO,
            duration: Timespan::from_secs_f64(180.0),
            volume: 0.8,
            pan: 0.0,
            fade_in: 2.0,
            fade_out: 3.0,
            fade_type: FadeType::SCurve,
            eq: None,
            routing: AudioRouting::default(),
        },
        Cue::Video {
            base: CueBase {
                trigger: TriggerMode::WithLast,
                ..cue_base(video, "Projection Intro", Some(group))
            },
            path: String::new(),
            start_time: Timespan::ZERO,
            duration: Timespan::from_secs_f64(42.0),
            volume: 0.7,
            pan: 0.0,
            fade_in: 1.0,
            fade_out: 1.0,
            fade_type: FadeType::SCurve,
            eq: None,
            routing: AudioRouting::default(),
            follow_mtc: false,
            mtc_start: Timespan::from_secs_f64(3600.0),
        },
        Cue::Text {
            base: CueBase {
                trigger: TriggerMode::AfterLast,
                ..cue_base(text, "Welcome Title", Some(group))
            },
            text: "Welcome to the Museum Gala".into(),
            font_size: 72.0,
            font_colour: SerializedColour::WHITE,
            fit: CanvasFit::Fit,
            font: String::new(),
        },
        Cue::Lighting {
            base: cue_base(Decimal::from(2), "House Lights Half", None),
            snapshot: Default::default(),
            fade_time: 2.5,
            fade_type: FadeType::SCurve,
        },
        Cue::Osc {
            base: cue_base(Decimal::from(3), "Arm Projection", None),
            command: "/projection/arm 1".into(),
        },
        Cue::Volume {
            base: cue_base(Decimal::from(4), "Fade Ambience", None),
            sound_qid: sound,
            fade_time: 4.0,
            volume: 0.25,
            fade_type: FadeType::SCurve,
        },
        Cue::Stop {
            base: cue_base(Decimal::from(5), "Stop Opening", None),
            stop_qid: group,
            stop_mode: StopMode::Immediate,
            fade_out_time: 1.0,
            fade_type: FadeType::Linear,
            stop_all: false,
        },
    ];
    show
}

fn populate_telemetry(state: &mut SharedState) {
    state.selected_cue_id = Some(Decimal::new(13, 1));
    state.active_cues = vec![
        ActiveCueInfo {
            qid: Decimal::new(11, 1),
            name: "Lobby Ambience".into(),
            volume: 0.8,
            paused: false,
            position_secs: 64.5,
            length_secs: Some(180.0),
            state: CueState::PlayingLooped,
        },
        ActiveCueInfo {
            qid: Decimal::new(12, 1),
            name: "Projection Intro".into(),
            volume: 0.7,
            paused: true,
            position_secs: 18.0,
            length_secs: Some(42.0),
            state: CueState::Paused,
        },
        ActiveCueInfo {
            qid: Decimal::from(2),
            name: "House Lights Half".into(),
            volume: 1.0,
            paused: false,
            position_secs: 1.2,
            length_secs: Some(2.5),
            state: CueState::Playing,
        },
    ];
    state.meter_data = GuiMeterData {
        peak_l_db: -12.0,
        peak_r_db: -14.0,
        rms_l_db: -20.0,
        rms_r_db: -22.0,
        clipped: false,
        limiter_gr_db: -2.0,
    };
    state.diagnostics = Diagnostics {
        app_version: "0.1.0-preview".into(),
        os: "Preview OS".into(),
        arch: "x86_64".into(),
        gpu_name: "Preview GPU".into(),
        gpu_backend: "Vulkan".into(),
        gpu_driver: "Demo Driver".into(),
        gpu_driver_info: "1.0.0".into(),
        ffmpeg_version: "8.0".into(),
        env_overrides: Vec::new(),
        outputs: vec![OutputDiagnostics {
            name: "Main Projector".into(),
            size: (1920, 1080),
            present_mode: "Fifo".into(),
            format: "Bgra8UnormSrgb".into(),
            refresh: "60.00 Hz".into(),
            fullscreen: true,
            presented_per_sec: 60.0,
        }],
        presented_per_sec: 60.0,
        starved_per_sec: 0.0,
        uploads_per_sec: 50.0,
        dropped_per_sec: 0.0,
        event_loop_per_sec: 240.0,
        consumer_error: None,
        video: Some(VideoDiagnostics {
            path: "projection-intro.mp4".into(),
            width: 1920,
            height: 1080,
            decode_path: "hardware (VideoToolbox)".into(),
            fallback_reason: None,
            timings: crate::VideoTimings {
                decode: crate::DecodeTiming::from_ms(4.2),
                hw_transfer: crate::DecodeTiming::from_ms(0.8),
                plane_copy: crate::DecodeTiming::from_ms(0.4),
                upload: crate::DecodeTiming::from_ms(0.5),
                conversion_submit: crate::DecodeTiming::from_ms(0.2),
            },
        }),
    };
    state.recorder_status = RecorderStatus {
        recording: true,
        elapsed_s: 12.5,
        event_count: 248,
        punched_count: 6,
        rx_packets: 750,
        error: None,
    };
    state.show_time = Some(78.32);
    state.show_paused = false;
    state.mtc_running = true;
    state.mtc_playing = true;
    state.mtc_timecode_secs = 3608.4;
    state.mtc_fps = 25.0;
    state.mtc_source = "MTC Preview".into();
    state.mtc_drift_ms = Some(4.0);
    state.next_timecode = Some((Decimal::from(3), 84.0));
    state.available_monitors = vec![
        MonitorId {
            name: "Operator Display".into(),
            width: 2560,
            height: 1440,
            pos_x: 0,
            pos_y: 0,
        },
        MonitorId {
            name: "Main Projector".into(),
            width: 1920,
            height: 1080,
            pos_x: 2560,
            pos_y: 0,
        },
    ];
    state.audio_devices = vec!["Built-in Output".into(), "Dante Virtual Soundcard".into()];
    state.audio_device_name = "Dante Virtual Soundcard".into();
}

/// Deterministic UI state populated with representative engine telemetry.
pub fn demo_state() -> SharedState {
    let mut state = SharedState {
        show_file: demo_show(),
        ..SharedState::default()
    };
    populate_telemetry(&mut state);
    state
}

/// An engine-free app suitable for previews and UI harnesses.
pub fn demo_app() -> CuePoolApp {
    let app = CuePoolApp::with_show_file(demo_show(), None);
    populate_telemetry(&mut app.state().lock().unwrap());
    app
}
