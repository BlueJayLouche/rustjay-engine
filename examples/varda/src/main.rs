fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting Varda v{}", env!("CARGO_PKG_VERSION"));

    #[cfg(all(feature = "egui", feature = "mixer"))]
    {
        rustjay_engine::run_with_egui_tabs(
            varda::VardaRootPlugin::new(),
            vec![
                Box::new(varda::ui::MixerTab),
                Box::new(varda::ui::DeckTab),
                Box::new(varda::ui::EffectsTab),
                Box::new(varda::ui::ModulationTab),
                Box::new(varda::ui::MidiTab),
                Box::new(varda::ui::StageTab),
                Box::new(varda::ui::OutputsTab),
                Box::new(varda::ui::SequencerTab),
                Box::new(varda::ui::InspectorTab),
            ],
        )
    }
    #[cfg(not(all(feature = "egui", feature = "mixer")))]
    {
        rustjay_engine::run(varda::VardaRootPlugin::new())
    }
}
