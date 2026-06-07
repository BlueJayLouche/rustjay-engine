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
            Box::new(varda::ui::MixerTab) as Box<dyn rustjay_engine::prelude::AnyEguiTab>,
            Box::new(varda::ui::DeckTab),
            Box::new(varda::ui::EffectsTab::default()),
            Box::new(varda::ui::MidiTab),
            Box::new(varda::ui::StageTab::new()),
            Box::new(varda::ui::OutputsTab::new()),
            Box::new(varda::ui::SequencerTab),
            Box::new(varda::ui::InspectorTab),
        ];
        let plugin = varda::VardaRootPlugin::new();
        // Share the live sync states with the projector stages so GUI edits
        // actually reach the render output.
        let warp_sync = plugin.warp_sync();
        let dome_sync = plugin.dome_sync();
        let edge_blend_sync = plugin.edge_blend_sync();

        // Load saved stage config (projector/headless list) so we can register
        // multiple projector windows at startup.
        let mut stage = plugin.default_state().stage;
        let workspace = varda::persistence::default_workspace();
        if let Ok(loaded) = workspace.load_stage() {
            stage.projectors = loaded.projectors;
            stage.headless_outputs = loaded.headless_outputs;
            log::info!(
                "[Main] loaded stage with {} projector(s), {} headless output(s)",
                stage.projectors.len(),
                stage.headless_outputs.len()
            );
        }

        // Create per-projector warp syncs so each output can render a
        // different assigned surface with its own warp.
        for proj in stage.projectors.iter_mut() {
            proj.warp_sync = Some(std::sync::Arc::new(std::sync::Mutex::new(
                varda::stage::WarpSync::default(),
            )));
        }

        rustjay_engine::run_with_projection_egui_tabs(plugin, tabs, move |sub| {
            use varda::stage::{VardaDomeStage, VardaEdgeBlendStage, VardaWarpStage};
            use winit::window::WindowAttributes;
            for (i, proj) in stage.projectors.iter().enumerate() {
                if !proj.enabled {
                    continue;
                }
                let attrs = WindowAttributes::default()
                    .with_title(format!("Varda Projector {} - {}", i + 1, proj.name))
                    .with_inner_size(winit::dpi::LogicalSize::new(proj.width, proj.height));
                if let Some(monitor_idx) = proj.fullscreen_monitor {
                    // winit monitor selection — iterate available monitors
                    // and pick the Nth one.  If the index is out of range,
                    // fall back to windowed.
                    // NOTE: event_loop is not available here; fullscreen
                    // is applied after window creation in a follow-up.
                    // For now we just size the window to the display.
                    log::info!(
                        "[Projector {}] requested fullscreen on monitor {}",
                        i,
                        monitor_idx
                    );
                }
                // Use per-projector warp sync if available, otherwise fall back
                // to the global warp sync (backward compatibility).
                let w = proj
                    .warp_sync
                    .clone()
                    .unwrap_or_else(|| warp_sync.clone());
                let d = dome_sync.clone();
                let e = edge_blend_sync.clone();
                sub.add_projector(attrs, move |device, format| {
                    vec![
                        Box::new(VardaDomeStage::new(device, format, d.clone())),
                        Box::new(VardaEdgeBlendStage::new(device, format, e.clone())),
                        Box::new(VardaWarpStage::new(device, format, w.clone())),
                    ]
                });
            }
            log::info!("Queued {} projector window(s)", sub.pending_len());
        })
    }
    #[cfg(all(feature = "egui", feature = "mixer", not(feature = "projection")))]
    {
        let tabs = vec![
            Box::new(varda::ui::MixerTab) as Box<dyn rustjay_engine::prelude::AnyEguiTab>,
            Box::new(varda::ui::DeckTab),
            Box::new(varda::ui::EffectsTab::default()),
            Box::new(varda::ui::MidiTab),
            Box::new(varda::ui::StageTab::new()),
            Box::new(varda::ui::OutputsTab::new()),
            Box::new(varda::ui::SequencerTab),
            Box::new(varda::ui::InspectorTab),
        ];
        rustjay_engine::run_with_egui_tabs(varda::VardaRootPlugin::new(), tabs)
    }
    #[cfg(not(all(feature = "egui", feature = "mixer")))]
    {
        rustjay_engine::run(varda::VardaRootPlugin::new())
    }
}
