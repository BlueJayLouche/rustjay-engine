# Routing Matrix

The routing matrix lets the user map FFT bands to parameters at runtime. When a band is routed to a parameter, its value is added to the parameter's base slider value each frame (after LFO modulation).

## How it works

At runtime, the user opens the Audio tab and assigns one of the 8 FFT bands to any declared parameter. They also set a gain and a response curve (linear, squared, etc.) for that mapping.

When `engine.get_param("intensity")` is called, the engine computes:

```
effective = base_value + lfo_contribution + (fft_band * gain * curve)
```

The result is clamped to the parameter's declared `[min, max]` range.

Your plugin code sees only the final value and doesn't need to know about the routing.

## In practice

Declare your parameters, use `get_param()` in `build_uniforms()`, and the routing matrix is available automatically:

```rust
fn parameters(&self) -> Vec<ParameterDescriptor> {
    vec![
        ParameterDescriptor::float("brightness", "Brightness", ParamCategory::Color, 0.0, 2.0, 1.0, 0.01),
        ParameterDescriptor::float("hue_shift",  "Hue Shift",  ParamCategory::Color, -180.0, 180.0, 0.0, 1.0),
    ]
}

fn build_uniforms(&self, s: &MyState, engine: &EngineState) -> MyUniforms {
    MyUniforms {
        brightness: engine.get_param("brightness").unwrap_or(s.brightness),
        hue_shift:  engine.get_param("hue_shift").unwrap_or(s.hue_shift),
    }
}
```

With this in place, the user can go to the Audio tab and set "bass band → brightness" with a gain of 0.8, and the brightness will pulse with the kick drum.

## Bypassing the routing matrix

If you want to read a raw FFT value and apply your own response curve, read from `engine.audio.fft` directly:

```rust
let kick_energy = engine.audio.fft[0].powf(0.5); // square-root curve
```

This skips the routing matrix entirely — useful for hard-coded audio reactivity where the user shouldn't need to configure anything.
