// Trivial fullscreen blit — GLSL ES 1.00.
precision mediump float;

varying vec2 v_uv;

uniform sampler2D u_src;

void main() {
    gl_FragColor = texture2D(u_src, v_uv);
}
