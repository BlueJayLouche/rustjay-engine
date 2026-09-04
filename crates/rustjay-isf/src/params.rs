//! Bridge ISF input declarations to rustjay-core parameter descriptors.

use std::collections::HashMap;

use isf::InputType;
use rustjay_core::{ParamCategory, ParameterDescriptor};

/// Convert a slice of ISF inputs into rustjay-engine [`ParameterDescriptor`]s.
///
/// Scalars (`Float`, `Bool`, `Long`) become one parameter each, and `Point2D`
/// becomes two — an X and a Y, because there is no single slider for a vec2 and
/// splitting it makes each axis MIDI- and LFO-mappable on its own.
///
/// Image, Color, Audio and AudioFFT are still skipped: the image and audio ones
/// are bound by the GPU pipeline rather than driven by a control, and no shader
/// in the corpus this was measured against declares a colour input.
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
            InputType::Point2d(p) => {
                // The `_x`/`_y` names are the ones `isf_inputs_to_default_values`
                // seeds and the uniform packer reads back for a `Vec2` field, so
                // the two parameters drive the shader's vec2 directly.
                let default = p.default.or(p.identity).unwrap_or([0.0, 0.0]);
                // Most point2D inputs declare no range. Normalised 0..1 is the
                // ISF convention, widened where needed so a default outside it
                // (a pixel coordinate, say) is still reachable on the slider.
                let min = p.min.unwrap_or([0.0, 0.0]);
                let max = p.max.unwrap_or([1.0, 1.0]);
                let label = input.label.clone().unwrap_or_else(|| input.name.clone());
                for (axis, i) in [("x", 0), ("y", 1)] {
                    let lo = min[i].min(default[i]);
                    let hi = max[i].max(default[i]);
                    params.push(ParameterDescriptor::float(
                        format!("{}_{axis}", input.name),
                        format!("{label} {}", axis.to_uppercase()),
                        ParamCategory::Custom("ISF".to_string()),
                        lo,
                        hi,
                        default[i],
                        ((hi - lo) / 100.0).max(0.001),
                    ));
                }
            }
            _ => {} // image, color, audio, audioFFT — skipped
        }
    }
    params
}

/// Build a map of ISF input name → default scalar value (as f32).
///
/// Bool is stored as 1.0 / 0.0; Long is stored as its integer value cast to f32.
/// Color inputs seed `name_r/g/b/a` component keys from DEFAULT (then IDENTITY);
/// a missing alpha component defaults to 1.0, missing RGB to 0.0. Point2D inputs
/// seed `name_x/y`. These keys are additive to `IsfState.values` (preset-serialised).
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
            InputType::Color(c) => {
                let arr = c.default.as_ref().or(c.identity.as_ref());
                // ISF color DEFAULTs are RGB or RGBA arrays; alpha defaults to 1.
                let comp = |i: usize, fallback: f32| {
                    arr.and_then(|a| a.get(i)).copied().unwrap_or(fallback)
                };
                values.insert(format!("{}_r", input.name), comp(0, 0.0));
                values.insert(format!("{}_g", input.name), comp(1, 0.0));
                values.insert(format!("{}_b", input.name), comp(2, 0.0));
                values.insert(format!("{}_a", input.name), comp(3, 1.0));
            }
            InputType::Point2d(p) => {
                let xy = p.default.or(p.identity).unwrap_or([0.0, 0.0]);
                values.insert(format!("{}_x", input.name), xy[0]);
                values.insert(format!("{}_y", input.name), xy[1]);
            }
            _ => {}
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point2d(name: &str, values: isf::InputPoint2d) -> isf::Input {
        isf::Input {
            name: name.to_string(),
            ty: InputType::Point2d(values),
            label: None,
        }
    }

    fn values(
        default: Option<[f32; 2]>,
        min: Option<[f32; 2]>,
        max: Option<[f32; 2]>,
    ) -> isf::InputPoint2d {
        isf::InputValues {
            default,
            min,
            max,
            identity: None,
        }
    }

    #[test]
    fn a_point_becomes_one_parameter_per_axis() {
        let params =
            isf_inputs_to_parameters(&[point2d("centre", values(Some([0.25, 0.75]), None, None))]);

        let ids: Vec<&str> = params.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["centre_x", "centre_y"]);
        assert_eq!(params[0].default, 0.25);
        assert_eq!(params[1].default, 0.75);
    }

    /// The names have to be the ones the uniform packer reads back, or the two
    /// sliders drive nothing.
    #[test]
    fn the_axes_are_named_the_way_the_defaults_are_seeded() {
        let input = point2d("pos", values(Some([0.1, 0.2]), None, None));
        let seeded = isf_inputs_to_default_values(std::slice::from_ref(&input));

        for param in isf_inputs_to_parameters(std::slice::from_ref(&input)) {
            assert_eq!(
                seeded.get(&param.id).copied(),
                Some(param.default),
                "{} is not seeded under the same key",
                param.id
            );
        }
    }

    #[test]
    fn a_declared_range_is_kept() {
        let params = isf_inputs_to_parameters(&[point2d(
            "pos",
            values(Some([0.0, 0.0]), Some([-2.0, -3.0]), Some([2.0, 3.0])),
        )]);

        assert_eq!((params[0].min, params[0].max), (-2.0, 2.0));
        assert_eq!((params[1].min, params[1].max), (-3.0, 3.0));
    }

    /// Most point2D inputs declare no range, and some then default well outside
    /// the normalised 0..1 assumed for them — a pixel coordinate, say. The
    /// slider has to be able to reach where the shader actually starts.
    #[test]
    fn an_out_of_range_default_widens_the_slider_instead_of_being_clamped() {
        let params =
            isf_inputs_to_parameters(&[point2d("mouse", values(Some([640.0, -12.0]), None, None))]);

        assert!(params[0].max >= 640.0, "x max was {}", params[0].max);
        assert!(params[1].min <= -12.0, "y min was {}", params[1].min);
        assert_eq!(params[0].default, 640.0);
        assert_eq!(params[1].default, -12.0);
    }

    #[test]
    fn images_and_audio_are_still_left_to_the_pipeline() {
        let params = isf_inputs_to_parameters(&[isf::Input {
            name: "inputImage".to_string(),
            ty: InputType::Image,
            label: None,
        }]);

        assert!(params.is_empty());
    }
}
