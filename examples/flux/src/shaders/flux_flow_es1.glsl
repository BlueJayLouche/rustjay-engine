// Per-pixel Lucas-Kanade optical flow — GLSL ES 1.00 (no UBOs).
precision mediump float;

varying vec2 v_uv;

uniform sampler2D u_curr;
uniform sampler2D u_prev;
uniform sampler2D u_prev_flow;

uniform float u_flow_lambda;
uniform float u_flow_smooth;
uniform float u_flow_scale;
uniform float u_audio_level;

// Resolution passed explicitly because GLSL ES 1.00 has no textureSize().
uniform vec2 u_resolution;

float luma(vec4 c) {
    return dot(c.rgb, vec3(0.299, 0.587, 0.114));
}

void main() {
    vec2 uv = v_uv;
    vec2 dx = vec2(1.0 / u_resolution.x, 0.0);
    vec2 dy = vec2(0.0, 1.0 / u_resolution.y);

    float ix = (luma(texture2D(u_curr, uv + dx)) - luma(texture2D(u_curr, uv - dx))) * 0.5;
    float iy = (luma(texture2D(u_curr, uv + dy)) - luma(texture2D(u_curr, uv - dy))) * 0.5;
    float it = luma(texture2D(u_curr, uv)) - luma(texture2D(u_prev, uv));

    float lambda = u_flow_lambda + 0.001;
    float denom  = ix * ix + iy * iy + lambda;
    float boost  = 1.0 + u_audio_level * 0.5;
    float vx = -it * ix / denom * u_flow_scale * boost;
    float vy = -it * iy / denom * u_flow_scale * boost;

    // Temporal smoothing: blend with previous flow (stored encoded)
    vec2 prev_enc     = texture2D(u_prev_flow, uv).xy;
    vec2 prev_decoded = (prev_enc - 0.5) * 2.0;
    vec2 smoothed     = mix(vec2(vx, vy), prev_decoded, u_flow_smooth);

    // Encode to [0,1]: 0.5 = zero velocity
    vec2 enc = clamp(smoothed * 0.5 + 0.5, vec2(0.0), vec2(1.0));
    float mag = clamp(length(smoothed), 0.0, 1.0);

    gl_FragColor = vec4(enc, mag, 1.0);
}
