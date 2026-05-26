# Template — HSB Colour Adjustment

`examples/template` is the reference starting point for rustjay-engine. It implements HSB (hue, saturation, brightness) colour grading in the simplest possible way — about 80 lines of Rust and a single WGSL shader — and demonstrates every core feature a real effect needs: parameters, audio reactivity, LFO targets, MIDI/OSC/web control, and presets.

```sh
cargo run -p template
```

Read this page alongside [Your First Effect](../getting-started/README.md) and [The EffectPlugin Trait](../core-concepts/README.md). Template is the canonical example those pages reference.

## What it does

Template applies three HSB adjustments to the video input:

- **Hue Shift** — rotates all hues by ±180°, wrapping at the colour wheel boundary
- **Saturation** — multiplies saturation; 0 = greyscale, 1 = original, 2 = oversaturated
- **Brightness** — multiplies value (HSV); 0 = black, 1 = original, 2 = overexposed

All three are live parameters — sliders in the control window, LFO targets, MIDI learnable, OSC addressable, and saved with presets.

## The Rust side

The full implementation (`src/main.rs`) is intentionally minimal:

```rust
struct HsbEffect;                         // no fields — all state lives below

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HsbUniforms {
    values: [f32; 4],                     // hue_shift, saturation, brightness, _pad
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct HsbState {
    hue_shift:  f32,
    saturation: f32,
    brightness: f32,
    enabled:    bool,
}

impl EffectPlugin for HsbEffect {
    type State    = HsbState;
    type Uniforms = HsbUniforms;

    fn app_name(&self) -> &str { "template" }

    fn default_state(&self) -> HsbState {
        HsbState { saturation: 1.0, brightness: 1.0, enabled: true, ..Default::default() }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float("hue_shift",  "Hue Shift",  ParamCategory::Color, -180.0, 180.0, 0.0,  1.0),
            ParameterDescriptor::float("saturation", "Saturation", ParamCategory::Color,    0.0,   2.0, 1.0, 0.01),
            ParameterDescriptor::float("brightness", "Brightness", ParamCategory::Color,    0.0,   2.0, 1.0, 0.01),
        ]
    }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/hsb.wgsl")
    }

    fn build_uniforms(&self, s: &HsbState, engine: &EngineState) -> HsbUniforms {
        if !s.enabled {
            return HsbUniforms { values: [0.0, 1.0, 1.0, 0.0] }; // passthrough
        }
        HsbUniforms { values: [
            engine.get_param("hue_shift").unwrap_or(s.hue_shift),
            engine.get_param("saturation").unwrap_or(s.saturation),
            engine.get_param("brightness").unwrap_or(s.brightness),
            0.0,
        ]}
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    rustjay_engine::run(HsbEffect)
}
```

### Things to notice

**`default_state()`** — `HsbState` derives `Default`, which would give `saturation: 0.0` and `brightness: 0.0` (a black screen). Overriding `default_state()` sets sensible starting values without requiring a custom `Default` impl for the whole struct.

**`engine.get_param()`** — returns the parameter's base slider value *plus* any active LFO and audio routing contributions, clamped to the declared range. Falling back to `s.hue_shift` etc. handles the case where the engine doesn't have a value yet (first frame before the parameter system initialises).

**The `enabled` guard** — returning a passthrough uniform (`[0, 1, 1, 0]`) when disabled lets the user bypass the effect without rebuilding the pipeline. The shader sees unmodified identity values.

**`ParamCategory::Color`** — places all three sliders in the built-in Color tab. Changing this to `ParamCategory::Motion` or `ParamCategory::Custom("name")` changes where they appear.

## The shader

`src/shaders/hsb.wgsl` does the colour conversion in three steps:

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(input_tex, input_sampler, in.texcoord);
    let adjusted = apply_hsb(color.rgb, hsb_params);
    return vec4<f32>(adjusted, color.a);
}
```

`apply_hsb()` converts RGB → HSV, applies the three adjustments, and converts back:

```wgsl
fn apply_hsb(rgb: vec3<f32>, params: HsbParams) -> vec3<f32> {
    var hsv = rgb_to_hsv(rgb);
    hsv.x = fract(hsv.x + params.values.x / 360.0); // hue rotation, wrapping
    hsv.y = clamp(hsv.y * params.values.y, 0.0, 1.0); // saturation scale
    hsv.z = clamp(hsv.z * params.values.z, 0.0, 1.0); // brightness scale
    return hsv_to_rgb(hsv);
}
```

`fract()` on the hue handles wrap-around — a shift of +350° and a shift of −10° produce the same result. `clamp()` on saturation and brightness prevents out-of-range values from producing invalid colours when LFO depth pushes a parameter past its declared bounds.

## Using template as a starting point

The recommended way to start a new effect:

```sh
cp -r examples/template my-effect
cd my-effect
# edit Cargo.toml name, then src/main.rs and src/shaders/
```

The minimum changes to make it your own:
1. Rename the structs (`HsbEffect` → `MyEffect`, etc.)
2. Change `app_name()` — this isolates config and presets from other effects
3. Replace `HsbUniforms` with your uniform layout
4. Replace `HsbState` with your state fields
5. Replace `parameters()` with your declared parameters
6. Rewrite `build_uniforms()` to fill your uniform struct
7. Rewrite the shader

Everything else — the control window, all built-in tabs, audio analysis, LFO system, MIDI, OSC, presets — works without any changes.
