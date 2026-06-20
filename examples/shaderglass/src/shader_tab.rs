//! Custom egui "Shader" tab: shader library (search), deck-style FX chain editor,
//! per-slot parameter controls, and profile save/load.

use std::path::PathBuf;

use rustjay_engine::prelude::*;

use crate::chain::{ChainCmd, ChainHandle};

pub struct ShaderTab {
    /// Library directory + file-picker default.
    pub shaders_dir: PathBuf,
    /// Cached library: (display name, path), sorted by name.
    pub library: Vec<(String, PathBuf)>,
    /// Current search filter.
    pub search: String,
    /// Shared handle to the render-side effect chain.
    pub chain: ChainHandle,
    /// Params from a loaded profile, applied once their slots register.
    pub pending_params: Option<Vec<(String, f32)>>,
}

impl ShaderTab {
    /// Scan `shaders_dir` for `.fs` files into the library, sorted by name.
    pub fn scan_library(dir: &PathBuf) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "fs"))
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|n| (n.to_string(), p.clone()))
            })
            .collect();
        out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        out
    }

    fn post(&self, cmd: ChainCmd) {
        if let Ok(mut g) = self.chain.cmds.lock() {
            g.push(cmd);
        }
    }

    /// Apply stashed profile params once every target id is registered.
    fn apply_pending(&mut self, engine: &mut EngineState) {
        let ready = self.pending_params.as_ref().is_some_and(|ps| {
            ps.iter()
                .all(|(id, _)| engine.param_descriptors.iter().any(|d| &d.id == id))
        });
        if ready {
            if let Some(ps) = self.pending_params.take() {
                for (id, val) in ps {
                    engine.set_param_base(&id, val);
                }
            }
        }
    }
}

impl AnyEguiTab for ShaderTab {
    fn name(&self) -> &str {
        "Shader"
    }

    fn draw(&mut self, ui: &mut egui::Ui, _app_state: &mut dyn std::any::Any, engine: &mut EngineState) {
        self.apply_pending(engine);

        // Host-feed every slot's `sourceAspect` from the live input resolution.
        let (iw, ih) = (engine.input.width.max(1), engine.input.height.max(1));
        let aspect = iw as f32 / ih as f32;
        let aspect_ids: Vec<String> = engine
            .param_descriptors
            .iter()
            .filter(|d| d.id.ends_with("sourceAspect"))
            .map(|d| d.id.clone())
            .collect();
        for id in aspect_ids {
            engine.set_param_base(&id, aspect);
        }

        // --- Library: search + add to chain ------------------------------
        ui.heading("Library");
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.search);
            if ui.button("Clear").clicked() {
                self.search.clear();
            }
        });
        let needle = self.search.to_lowercase();
        let mut add: Option<PathBuf> = None;
        egui::ScrollArea::vertical()
            .id_salt("library")
            .max_height(160.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (name, path) in &self.library {
                    if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    if ui.button(format!("+ {name}")).clicked() {
                        add = Some(path.clone());
                    }
                }
            });
        if let Some(p) = add {
            self.post(ChainCmd::Add(p));
        }

        ui.separator();

        // --- FX chain editor ---------------------------------------------
        ui.heading("FX Chain");
        let roster = self.chain.roster.lock().map(|r| r.clone()).unwrap_or_default();
        let last = roster.len().saturating_sub(1);
        for (i, slot) in roster.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}. {}", i + 1, slot.name));
                if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                    self.post(ChainCmd::Move(slot.prefix.clone(), -1));
                }
                if ui.add_enabled(i < last, egui::Button::new("▼")).clicked() {
                    self.post(ChainCmd::Move(slot.prefix.clone(), 1));
                }
                if ui.add_enabled(roster.len() > 1, egui::Button::new("✕")).clicked() {
                    self.post(ChainCmd::Remove(slot.prefix.clone()));
                }
            });
        }

        // --- Profiles ----------------------------------------------------
        ui.horizontal(|ui| {
            if ui.button("Save Profile…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ShaderGlass Profile", &["json"])
                    .set_file_name("profile.json")
                    .save_file()
                {
                    let names: Vec<String> = roster.iter().map(|s| s.name.clone()).collect();
                    let prof = crate::profile::Profile::capture(&names, engine);
                    if let Err(e) = prof.save(&path) {
                        log::error!("Save profile failed: {e}");
                    }
                }
            }
            if ui.button("Load Profile…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ShaderGlass Profile", &["json"])
                    .pick_file()
                {
                    match crate::profile::Profile::load(&path) {
                        Ok(prof) => self.load_profile(prof, engine),
                        Err(e) => log::error!("Load profile failed: {e}"),
                    }
                }
            }
        });

        ui.separator();

        // --- Output + parameters -----------------------------------------
        let (ow, oh) = (engine.output_width.max(1), engine.output_height.max(1));
        ui.label(format!(
            "Source {iw}×{ih}  →  Output {ow}×{oh}   (Shift+F = fullscreen)"
        ));

        let descriptors = engine.param_descriptors.clone();
        for slot in &roster {
            let any = descriptors.iter().any(|d| d.id.starts_with(&slot.prefix));
            if !any {
                continue;
            }
            egui::CollapsingHeader::new(&slot.name)
                .default_open(true)
                .id_salt(&slot.prefix)
                .show(ui, |ui| {
                    for desc in descriptors.iter() {
                        if !desc.id.starts_with(&slot.prefix) || desc.id.ends_with("sourceAspect") {
                            continue;
                        }
                        draw_param(ui, engine, desc);
                    }
                });
        }
    }
}

impl ShaderTab {
    fn load_profile(&mut self, prof: crate::profile::Profile, engine: &mut EngineState) {
        prof.restore_source(engine);
        let paths: Vec<PathBuf> = prof
            .chain
            .iter()
            .map(|name| self.shaders_dir.join(format!("{name}.fs")))
            .filter(|p| p.exists())
            .collect();
        if !paths.is_empty() {
            self.post(ChainCmd::Replace(paths));
        }
        self.pending_params = Some(prof.params);
    }
}

/// One parameter control, reading/writing through the engine.
fn draw_param(ui: &mut egui::Ui, engine: &mut EngineState, desc: &ParameterDescriptor) {
    let current = engine.get_param(&desc.id).unwrap_or(desc.default);
    match &desc.param_type {
        ParamType::Float => {
            let mut v = current;
            if ui
                .add(egui::Slider::new(&mut v, desc.min..=desc.max).text(&desc.name))
                .changed()
            {
                engine.set_param_base(&desc.id, v);
            }
        }
        ParamType::Int => {
            let mut v = current as i32;
            if ui
                .add(egui::Slider::new(&mut v, desc.min as i32..=desc.max as i32).text(&desc.name))
                .changed()
            {
                engine.set_param_base(&desc.id, v as f32);
            }
        }
        ParamType::Bool => {
            let mut on = current >= 0.5;
            if ui.checkbox(&mut on, &desc.name).changed() {
                engine.set_param_base(&desc.id, if on { 1.0 } else { 0.0 });
            }
        }
        ParamType::Enum { variants } => {
            let mut idx = current as usize;
            let sel = variants.get(idx).map(String::as_str).unwrap_or("?");
            egui::ComboBox::from_label(&desc.name)
                .selected_text(sel)
                .show_ui(ui, |ui| {
                    for (i, name) in variants.iter().enumerate() {
                        ui.selectable_value(&mut idx, i, name);
                    }
                });
            if idx as f32 != current {
                engine.set_param_base(&desc.id, idx as f32);
            }
        }
    }
}
