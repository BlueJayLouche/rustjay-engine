//! OpenAPI schema generation and Swagger UI.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "RustJay API", version = "0.1.0", description = "Optional REST/OpenAPI layer for rustjay-engine"),
    paths(
        // System
        crate::routes::system::health,
        crate::routes::system::get_state,
        crate::routes::system::set_param,
        // Input
        crate::routes::system::input_start_webcam,
        crate::routes::system::input_stop,
        crate::routes::system::input_refresh,
        // Output
        crate::routes::system::output_start_ndi,
        crate::routes::system::output_stop_ndi,
        crate::routes::system::output_resize,
        crate::routes::system::output_start_syphon,
        crate::routes::system::output_stop_syphon,
        crate::routes::system::output_start_spout,
        crate::routes::system::output_stop_spout,
        crate::routes::system::output_start_v4l2,
        crate::routes::system::output_stop_v4l2,
        // Audio
        crate::routes::system::audio_start,
        crate::routes::system::audio_stop,
        crate::routes::system::audio_refresh,
        crate::routes::system::audio_select_device,
        crate::routes::system::audio_set_fft_size,
        // MIDI
        crate::routes::system::midi_refresh,
        crate::routes::system::midi_select_device,
        crate::routes::system::midi_disconnect,
        crate::routes::system::midi_learn,
        crate::routes::system::midi_learn_cancel,
        crate::routes::system::midi_unlearn,
        // OSC
        crate::routes::system::osc_start,
        crate::routes::system::osc_stop,
        crate::routes::system::osc_set_port,
        // Presets
        crate::routes::system::preset_save,
        crate::routes::system::preset_load,
        crate::routes::system::preset_delete,
        // Link
        crate::routes::system::link_enable,
        crate::routes::system::link_disable,
        crate::routes::system::link_set_quantum,
        // ProDJ
        crate::routes::system::prodj_start,
        crate::routes::system::prodj_stop,
        // Mixer
        crate::routes::system::mixer_crossfader,
        crate::routes::system::mixer_master_opacity,
        // Modulation
        crate::routes::system::modulation_lfo_set,
        crate::routes::system::modulation_lfo_enable,
        crate::routes::system::modulation_audio_route,
        crate::routes::system::modulation_audio_unroute,
        crate::routes::system::modulation_tap_tempo,
        // App (generic, app-agnostic)
        crate::routes::app::get_app_state,
        crate::routes::app::list_params,
        crate::routes::app::set_param_by_path,
    ),
    tags(
        (name = "System", description = "Health, state, parameters"),
        (name = "Input", description = "Video input control"),
        (name = "Output", description = "Video output control"),
        (name = "Audio", description = "Audio analysis and device control"),
        (name = "MIDI", description = "MIDI device and mapping control"),
        (name = "OSC", description = "OSC server control"),
        (name = "Presets", description = "Preset save/load/delete"),
        (name = "Link", description = "Ableton Link sync control"),
        (name = "ProDJ", description = "ProDJ Link sync control"),
        (name = "Mixer", description = "Mixer crossfader and opacity"),
        (name = "Modulation", description = "LFO and audio-routing modulation control"),
        (name = "App", description = "App-published state snapshot and hierarchical param I/O"),
    )
)]
/// OpenAPI documentation container.
pub struct ApiDoc;
