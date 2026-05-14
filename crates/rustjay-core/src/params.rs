//! # Parameter Descriptor System
//!
//! Effect-agnostic parameter declarations that drive UI generation,
//! LFO targets, audio routing targets, and control protocol mappings.

use serde::{Deserialize, Serialize};

/// Describes one parameter exposed by an effect.
///
/// The engine uses this metadata to auto-generate:
/// - Tab UI (sliders, checkboxes, dropdowns)
/// - LFO target dropdowns
/// - Audio routing target dropdowns
/// - OSC address mappings
/// - MIDI learn targets
/// - Web remote controls
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterDescriptor {
    /// Unique identifier for this parameter (e.g. `"red_delay"`, `"hue_shift"`).
    pub id: String,
    /// Human-readable display name (e.g. `"Red Delay"`, `"Hue Shift"`).
    pub name: String,
    /// Category that drives which tab renders this parameter.
    pub category: ParamCategory,
    /// Data type — determines the UI widget and whether the parameter
    /// can be targeted by LFOs and audio routing.
    pub param_type: ParamType,
    /// Minimum value (for Float / Int).
    pub min: f32,
    /// Maximum value (for Float / Int).
    pub max: f32,
    /// Default / starting value.
    pub default: f32,
    /// Step size for slider increments.
    pub step: f32,
}

impl ParameterDescriptor {
    /// Create a new float parameter descriptor.
    pub fn float(
        id: impl Into<String>,
        name: impl Into<String>,
        category: ParamCategory,
        min: f32,
        max: f32,
        default: f32,
        step: f32,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category,
            param_type: ParamType::Float,
            min,
            max,
            default,
            step,
        }
    }

    /// Create a new integer parameter descriptor.
    pub fn int(
        id: impl Into<String>,
        name: impl Into<String>,
        category: ParamCategory,
        min: i32,
        max: i32,
        default: i32,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category,
            param_type: ParamType::Int,
            min: min as f32,
            max: max as f32,
            default: default as f32,
            step: 1.0,
        }
    }

    /// Create a new boolean parameter descriptor.
    pub fn bool(
        id: impl Into<String>,
        name: impl Into<String>,
        category: ParamCategory,
        default: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category,
            param_type: ParamType::Bool,
            min: 0.0,
            max: 1.0,
            default: if default { 1.0 } else { 0.0 },
            step: 1.0,
        }
    }

    /// Create a new enum parameter descriptor.
    pub fn enum_param(
        id: impl Into<String>,
        name: impl Into<String>,
        category: ParamCategory,
        variants: Vec<String>,
        default_index: usize,
    ) -> Self {
        let max = (variants.len().saturating_sub(1)).max(0) as f32;
        Self {
            id: id.into(),
            name: name.into(),
            category,
            param_type: ParamType::Enum { variants },
            min: 0.0,
            max,
            default: default_index as f32,
            step: 1.0,
        }
    }

    /// Whether this parameter can be targeted by LFOs and audio routing.
    pub fn is_modulatable(&self) -> bool {
        matches!(self.param_type, ParamType::Float | ParamType::Int)
    }

    /// Get the default value as a boolean.
    pub fn default_bool(&self) -> bool {
        self.default >= 0.5
    }

    /// Get the default value as an integer.
    pub fn default_int(&self) -> i32 {
        self.default as i32
    }
}

/// Parameter category — drives which built-in tab renders the parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParamCategory {
    /// Color / HSB adjustment parameters → Color tab.
    Color,
    /// Motion / spatial effect parameters → Motion tab.
    Motion,
    /// Audio-related parameters.
    Audio,
    /// Output configuration parameters.
    Output,
    /// General settings parameters.
    Settings,
    /// Custom category with a user-defined tab name.
    Custom(String),
}

impl ParamCategory {
    /// Human-readable category name.
    pub fn name(&self) -> String {
        match self {
            ParamCategory::Color => "Color".to_string(),
            ParamCategory::Motion => "Motion".to_string(),
            ParamCategory::Audio => "Audio".to_string(),
            ParamCategory::Output => "Output".to_string(),
            ParamCategory::Settings => "Settings".to_string(),
            ParamCategory::Custom(s) => s.clone(),
        }
    }

    /// Tab identifier — used for matching against `GuiTab` or custom tabs.
    pub fn tab_name(&self) -> String {
        self.name()
    }
}

/// Parameter type — determines the UI widget and modulatability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamType {
    /// Floating-point value — rendered as a slider.
    /// Can be targeted by LFOs and audio routing.
    Float,
    /// Integer value — rendered as an integer slider.
    /// Can be targeted by LFOs and audio routing.
    Int,
    /// Boolean value — rendered as a checkbox.
    /// Not modulatable.
    Bool,
    /// Enumerated value — rendered as a dropdown.
    /// Not modulatable.
    Enum { variants: Vec<String> },
}

impl ParamType {
    /// Human-readable type name.
    pub fn name(&self) -> &'static str {
        match self {
            ParamType::Float => "Float",
            ParamType::Int => "Int",
            ParamType::Bool => "Bool",
            ParamType::Enum { .. } => "Enum",
        }
    }
}
