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
            Box::new(varda::ui::EffectsTab),
            Box::new(varda::ui::ModulationTab),
            Box::new(varda::ui::MidiTab),
            Box::new(varda::ui::StageTab::new()),
            Box::new(varda::ui::OutputsTab),
            Box::new(varda::ui::SequencerTab),
            Box::new(varda::ui::InspectorTab),
        ];
        let plugin = varda::VardaRootPlugin::new();
        // Share the live sync states with the projector stages so GUI edits
        // actually reach the render output.
        let warp_sync = plugin.warp_sync();
        let dome_sync = plugin.dome_sync();
        let edge_blend_sync = plugin.edge_blend_sync();
        rustjay_engine::run_with_projection_egui_tabs(
            plugin,
            tabs,
            move |sub| {
                use varda::stage::{VardaDomeStage, VardaEdgeBlendStage, VardaWarpStage};
                use winit::window::WindowAttributes;
                sub.add_projector(
                    WindowAttributes::default()
                        .with_title("Varda Projector 1")
                        .with_inner_size(winit::dpi::LogicalSize::new(640u32, 480u32)),
                    move |device, format| {
                        // Pipeline order: domemaster reproject → edge-blend seam correction → warp.
                        // This matches the physical signal flow for a domed multi-projector rig.
                        vec![
                            Box::new(VardaDomeStage::new(device, format, dome_sync.clone())),
                            Box::new(VardaEdgeBlendStage::new(device, format, edge_blend_sync.clone())),
                            Box::new(VardaWarpStage::new(device, format, warp_sync.clone())),
                        ]
                    },
                );
                log::info!("Queued {} projector window(s)", sub.pending_len());
            },
        )
    }
    #[cfg(all(feature = "egui", feature = "mixer", not(feature = "projection")))]
    {
        let tabs = vec![
            Box::new(varda::ui::MixerTab) as Box<dyn rustjay_engine::prelude::AnyEguiTab>,
            Box::new(varda::ui::DeckTab),
            Box::new(varda::ui::EffectsTab),
            Box::new(varda::ui::ModulationTab),
            Box::new(varda::ui::MidiTab),
            Box::new(varda::ui::StageTab::new()),
            Box::new(varda::ui::OutputsTab),
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
