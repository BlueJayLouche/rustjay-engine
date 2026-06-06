//! Bridge ISF input declarations to rustjay-core parameter descriptors.

use std::collections::HashMap;

use isf::InputType;
use rustjay_core::{ParamCategory, ParameterDescriptor};

/// Convert a slice of ISF inputs into rustjay-engine [`ParameterDescriptor`]s.
///
/// Only scalar types are exposed as parameters (`Float`, `Bool`, `Long`).
/// Image, Color, Point2D, Audio, and AudioFFT inputs are skipped — they are
/// handled automatically by the GPU pipeline or UI.
pub fn isf_inputs_to_parameters(inputs: &[isf::Input]) -> Vec<ParameterDescriptor> {
    let mut params = Vec::new();
    for input in inputs {
        match &input.ty {
            InputType::Float(f) => {
                let min = f.min.unwrap_or(0.0);
                let max = f.max.unwrap_or(1.0);
                let default = f.default.unwrap_or(0.0);
                let step = ((max - min) / 100.0).max(0.001);
                let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                params.push(ParameterDescriptor::float(
                    &input.name,
                    label,
                    ParamCategory::Custom("ISF".to_string()),
                    min,
                    max,
                    default,
                    step,
                ));
            }
            InputType::Bool(b) => {
                let default = b.default.unwrap_or(false);
                let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                params.push(ParameterDescriptor::bool(
                    &input.name,
                    label,
                    ParamCategory::Custom("ISF".to_string()),
                    default,
                ));
            }
            InputType::Long(l) => {
                let min = l.min.unwrap_or(0);
                let max = l.max.unwrap_or(10);
                let default = l.default.unwrap_or(0);
                let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                params.push(ParameterDescriptor::int(
                    &input.name,
                    label,
                    ParamCategory::Custom("ISF".to_string()),
                    min,
                    max,
                    default,
                ));
            }
            _ => {} // image, color, point2D, audio, audioFFT — skipped
        }
    }
    params
}

/// Build a map of ISF input name → default scalar value (as f32).
///
/// Bool is stored as 1.0 / 0.0; Long is stored as its integer value cast to f32.
pub fn isf_inputs_to_default_values(inputs: &[isf::Input]) -> HashMap<String, f32> {
    let mut values = HashMap::new();
    for input in inputs {
        match &input.ty {
            InputType::Float(f) => {
                values.insert(input.name.clone(), f.default.unwrap_or(0.0));
            }
            InputType::Bool(b) => {
                values.insert(
                    input.name.clone(),
                    if b.default.unwrap_or(false) { 1.0 } else { 0.0 },
                );
            }
            InputType::Long(l) => {
                values.insert(input.name.clone(), l.default.unwrap_or(0) as f32);
            }
            _ => {}
        }
    }
    values
}
