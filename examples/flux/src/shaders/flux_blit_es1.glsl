// Fullscreen blit with optional HSB correction — GLSL ES 1.00.
// u_hsb = (hue_shift_degrees, saturation_mult, brightness_mult, enabled: 0|1)
// When u_hsb.w < 0.5 the shader returns early — uniform-flow-control branch,
// so the whole quad takes the same path and no extra ALU runs.
precision mediump float;

varying vec2 v_uv;

uniform sampler2D u_src;
uniform vec4      u_hsb;

// Branchless RGB→HSV (Iñigo Quílez)
vec3 rgb2hsv(vec3 c) {
    vec4 K  = vec4(0.0, -1.0/3.0, 2.0/3.0, -1.0);
    vec4 p  = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q  = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

// Branchless HSV→RGB
vec3 hsv2rgb(vec3 c) {
    vec3 p = abs(fract(vec3(c.x) + vec3(0.0, 2.0/3.0, 1.0/3.0)) * 6.0 - vec3(3.0));
    return c.z * mix(vec3(1.0), clamp(p - vec3(1.0), 0.0, 1.0), c.y);
}

void main() {
    vec4 color = texture2D(u_src, v_uv);

    if (u_hsb.w < 0.5) {
        gl_FragColor = color;
        return;
    }

    vec3 hsv = rgb2hsv(color.rgb);
    hsv.x = fract(hsv.x + u_hsb.x / 360.0);
    hsv.y = clamp(hsv.y * u_hsb.y, 0.0, 1.0);
    hsv.z = clamp(hsv.z * u_hsb.z, 0.0, 1.0);
    gl_FragColor = vec4(hsv2rgb(hsv), color.a);
}
