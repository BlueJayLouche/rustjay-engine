#[cfg(all(feature = "egui", feature = "mixer", feature = "projection"))]
use rustjay_engine::EffectPlugin;

fn main() -> anyhow::Result<()> {
    // Warn by default: from_default_env() alone leaves the level at Error with
    // RUST_LOG unset, which hides every warning the engine emits. parse_default_env()
    // last so RUST_LOG still overrides both the default and the module filters below.
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    // Title-bar/taskbar icon on Windows + X11; the macOS bundle uses AppIcon.icns.
    rustjay_engine::set_window_icon(include_bytes!("../packaging/icon-256.png"));

    log::info!("Starting KOVVBOJ v{}", env!("CARGO_PKG_VERSION"));

    #[cfg(all(feature = "egui", feature = "mixer", feature = "projection"))]
    {
        let plugin = kovvboj::KovvbojRootPlugin::new();
        // Share the live sync states with the projector stages so GUI edits
        // actually reach the render output.
        let dome_sync = plugin.dome_sync();
        let edge_blend_sync = plugin.edge_blend_sync();

        // Load saved stage config (projector/headless list) so we can register
        // multiple projector windows at startup.
        let mut stage = plugin.default_state().stage;
        let workspace = kovvboj::persistence::default_workspace();
        if let Ok(loaded) = workspace.load_stage() {
            stage.projectors = loaded.projectors;
            stage.headless_outputs = loaded.headless_outputs;
            log::info!(
                "[Main] loaded stage with {} projector(s), {} headless output(s)",
                stage.projectors.len(),
                stage.headless_outputs.len()
            );
        }

        // Ensure plugin-level syncs match projector count so stages and app state
        // share the same Arcs.
        plugin.ensure_source_syncs(stage.projectors.len());
        plugin.ensure_rotation_syncs(stage.projectors.len());

        // Clone syncs for the closure (plugin will be moved into the engine).
        let source_syncs = plugin.source_syncs();
        let rotation_syncs = plugin.rotation_syncs();
        log::info!(
            "[Main] captured syncs: source={}, rotation={}",
            source_syncs.len(),
            rotation_syncs.len()
        );

        rustjay_engine::run_with_projection_egui_shell(
            plugin,
            Box::new(kovvboj::shell::KovvbojShell::new()),
            move |sub| {
            for (i, proj) in stage.projectors.iter().enumerate() {
                if !proj.enabled {
                    continue;
                }
                let attrs = rustjay_engine::window_attributes()
                    .with_title(format!("KOVVBOJ Projector {} - {}", i + 1, proj.name))
                    .with_inner_size(winit::dpi::LogicalSize::new(proj.width, proj.height));
                if let Some(monitor_idx) = proj.fullscreen_monitor {
                    log::info!(
                        "[Projector {}] requested fullscreen on monitor {}",
                        i,
                        monitor_idx
                    );
                }
                let d = dome_sync.clone();
                let e = edge_blend_sync.clone();
                let s = source_syncs.get(i).cloned().unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::Mutex::new(kovvboj::stage::SourceSync::default()))
                });
                let r = rotation_syncs.get(i).cloned().unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::Mutex::new(rustjay_projection::RotationSync::default()))
                });
                sub.add_projector(attrs, proj.fullscreen_monitor, move |device, format| {
                    kovvboj::stage::projector_stages(device, format, &s, &d, &e, &r)
                });
            }
            log::info!("Queued {} projector window(s)", sub.pending_len());
        })
    }
    #[cfg(all(feature = "egui", feature = "mixer", not(feature = "projection")))]
    {
        rustjay_engine::run_with_egui_shell(
            kovvboj::KovvbojRootPlugin::new(),
            Box::new(kovvboj::shell::KovvbojShell::new()),
        )
    }
    #[cfg(not(all(feature = "egui", feature = "mixer")))]
    {
        rustjay_engine::run(kovvboj::KovvbojRootPlugin::new())
    }
}
