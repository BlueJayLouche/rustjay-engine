// Warp webcam by flow field and accumulate feedback — GLSL ES 1.00 (no UBOs).
precision mediump float;

varying vec2 v_uv;

uniform sampler2D u_input;
uniform sampler2D u_flow;
uniform sampler2D u_accum;

uniform float u_warp_strength;
uniform float u_drift_strength;
uniform float u_feedback_decay;
uniform float u_webcam_mix;
uniform float u_flow_viz;
uniform float u_flow_viz_scale;
uniform float u_bass;
uniform float u_treble;

vec3 hsv_to_rgb(float h, float s, float v) {
    float c = v * s;
    float x = c * (1.0 - abs(mod(h * 6.0, 2.0) - 1.0));
    float m = v - c;
    vec3 rgb;
    float hi = floor(h * 6.0);
    if      (hi < 1.0) rgb = vec3(c, x, 0.0);
    else if (hi < 2.0) rgb = vec3(x, c, 0.0);
    else if (hi < 3.0) rgb = vec3(0.0, c, x);
    else if (hi < 4.0) rgb = vec3(0.0, x, c);
    else if (hi < 5.0) rgb = vec3(x, 0.0, c);
    else               rgb = vec3(c, 0.0, x);
    return rgb + m;
}

// fract() in GLSL ES 1.00 is built-in; manual wrap for clarity.
vec2 wrap(vec2 uv) {
    return uv - floor(uv);
}

void main() {
    vec2 uv   = v_uv;
    vec2 enc  = texture2D(u_flow, uv).xy;
    vec2 flow = (enc - 0.5) * 2.0;
    float mag = length(flow);

    float warp = u_warp_strength * (1.0 + u_bass * 0.8);

    vec4 webcam = texture2D(u_input, wrap(uv - flow * warp));
    vec4 accum  = texture2D(u_accum, wrap(uv + flow * u_drift_strength));

    float decay = u_feedback_decay * (1.0 - u_treble * 0.1);
    vec4  faded = accum * clamp(decay, 0.0, 0.9999);
    vec4  color = mix(faded, webcam, clamp(u_webcam_mix, 0.0, 1.0));

    if (u_flow_viz > 0.0 && mag > 0.001) {
        float angle     = atan(flow.y, flow.x) / (2.0 * 3.14159265) + 0.5;
        float brightness = clamp(mag * u_flow_viz_scale, 0.0, 1.0);
        vec4  flow_col  = vec4(hsv_to_rgb(angle, 0.85, brightness), 1.0);
        float blend_amt = u_flow_viz * clamp(brightness, 0.0, 1.0);
        color = mix(color, flow_col, blend_amt);
    }

    gl_FragColor = color;
}
