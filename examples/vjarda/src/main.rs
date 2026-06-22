#[cfg(all(feature = "egui", feature = "mixer", feature = "projection"))]
use rustjay_engine::EffectPlugin;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting Varda v{}", env!("CARGO_PKG_VERSION"));

    #[cfg(all(feature = "egui", feature = "mixer", feature = "projection"))]
    {
        let tabs = vec![
            Box::new(vjarda::ui::MixerTab::default()) as Box<dyn rustjay_engine::prelude::AnyEguiTab>,
            Box::new(vjarda::ui::DeckTab::default()),
            Box::new(vjarda::ui::EffectsTab::default()),
            Box::new(vjarda::ui::MidiTab),
            Box::new(vjarda::ui::StageTab::new()),
            Box::new(vjarda::ui::OutputsTab::new()),
            Box::new(vjarda::ui::SequencerTab),
            Box::new(vjarda::ui::InspectorTab),
            #[cfg(feature = "webcam")]
            Box::new(vjarda::ui::LedMapTab::new()),
        ];
        let plugin = vjarda::VardaRootPlugin::new();
        // Share the live sync states with the projector stages so GUI edits
        // actually reach the render output.
        let dome_sync = plugin.dome_sync();
        let edge_blend_sync = plugin.edge_blend_sync();

        // Load saved stage config (projector/headless list) so we can register
        // multiple projector windows at startup.
        let mut stage = plugin.default_state().stage;
        let workspace = vjarda::persistence::default_workspace();
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
        plugin.ensure_warp_syncs(stage.projectors.len());

        // Clone syncs for the closure (plugin will be moved into the engine).
        let source_syncs = plugin.source_syncs();
        let rotation_syncs = plugin.rotation_syncs();
        let warp_syncs = plugin.warp_syncs();
        log::info!(
            "[Main] captured syncs: source={}, rotation={}, warp={}",
            source_syncs.len(),
            rotation_syncs.len(),
            warp_syncs.len()
        );

        rustjay_engine::run_with_projection_egui_tabs(plugin, tabs, move |sub| {
            use vjarda::stage::{VardaDomeStage, VardaEdgeBlendStage, VardaSourceStage, VardaWarpStage};
            use winit::window::WindowAttributes;
            for (i, proj) in stage.projectors.iter().enumerate() {
                if !proj.enabled {
                    continue;
                }
                let attrs = WindowAttributes::default()
                    .with_title(format!("Varda Projector {} - {}", i + 1, proj.name))
                    .with_inner_size(winit::dpi::LogicalSize::new(proj.width, proj.height));
                if let Some(monitor_idx) = proj.fullscreen_monitor {
                    log::info!(
                        "[Projector {}] requested fullscreen on monitor {}",
                        i,
                        monitor_idx
                    );
                }
                let w = warp_syncs.get(i).cloned().unwrap_or_else(|| {
                    log::warn!("[Projector {}] warp_syncs missing, using default", i);
                    std::sync::Arc::new(std::sync::Mutex::new(vjarda::stage::WarpSync::default()))
                });
                log::info!("[Projector {}] warp_sync ptr={:p}", i, std::sync::Arc::as_ptr(&w));
                let d = dome_sync.clone();
                let e = edge_blend_sync.clone();
                let s = source_syncs.get(i).cloned().unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::Mutex::new(vjarda::stage::SourceSync::default()))
                });
                let r = rotation_syncs.get(i).cloned().unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::Mutex::new(rustjay_projection::RotationSync::default()))
                });
                sub.add_projector(attrs, proj.fullscreen_monitor, move |device, format| {
                    vec![
                        Box::new(VardaSourceStage::new(device, format, s.clone())),
                        Box::new(VardaDomeStage::new(device, format, d.clone())),
                        Box::new(VardaEdgeBlendStage::new(device, format, e.clone())),
                        Box::new(VardaWarpStage::new(device, format, w.clone())),
                        Box::new(rustjay_projection::RotationStage::new(device, format, r.clone())),
                    ]
                });
            }
            log::info!("Queued {} projector window(s)", sub.pending_len());
        })
    }
    #[cfg(all(feature = "egui", feature = "mixer", not(feature = "projection")))]
    {
        let tabs = vec![
            Box::new(vjarda::ui::MixerTab::default()) as Box<dyn rustjay_engine::prelude::AnyEguiTab>,
            Box::new(vjarda::ui::DeckTab::default()),
            Box::new(vjarda::ui::EffectsTab::default()),
            Box::new(vjarda::ui::MidiTab),
            Box::new(vjarda::ui::StageTab::new()),
            Box::new(vjarda::ui::OutputsTab::new()),
            Box::new(vjarda::ui::SequencerTab),
            Box::new(vjarda::ui::InspectorTab),
            #[cfg(feature = "webcam")]
            Box::new(vjarda::ui::LedMapTab::new()),
        ];
        rustjay_engine::run_with_egui_tabs(vjarda::VardaRootPlugin::new(), tabs)
    }
    #[cfg(not(all(feature = "egui", feature = "mixer")))]
    {
        rustjay_engine::run(vjarda::VardaRootPlugin::new())
    }
}
