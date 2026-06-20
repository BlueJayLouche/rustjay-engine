/*{
    "CREDIT": "Shadertoy port",
    "DESCRIPTION": "Luscious Ripples — life-fluid raymarched field",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Generator"],
    "INPUTS": [
        {"NAME": "speed",      "LABEL": "Animation Speed", "TYPE": "float", "MIN": 0.0,  "MAX": 3.0, "DEFAULT": 1.0},
        {"NAME": "pulse",      "LABEL": "Pulsation",       "TYPE": "float", "MIN": 0.0,  "MAX": 1.0, "DEFAULT": 0.4},
        {"NAME": "flow",       "LABEL": "Fluid Motion",    "TYPE": "float", "MIN": 0.0,  "MAX": 1.0, "DEFAULT": 0.3},
        {"NAME": "density",    "LABEL": "Density",         "TYPE": "float", "MIN": 0.01, "MAX": 0.2, "DEFAULT": 0.05},
        {"NAME": "colorSpeed", "LABEL": "Color Shift",     "TYPE": "float", "MIN": 0.0,  "MAX": 2.0, "DEFAULT": 0.5},
        {"NAME": "colorBoost", "LABEL": "Brightness",      "TYPE": "float", "MIN": 0.5,  "MAX": 3.0, "DEFAULT": 2.0}
    ]
}*/


//----------------------
// Noise / FBM
//----------------------

float hash(vec3 p) {
    return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453);
}

float noise(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    f = f*f*(3.0-2.0*f);

    float n = mix(mix(mix( hash(i + vec3(0,0,0)), hash(i + vec3(1,0,0)), f.x),
                      mix( hash(i + vec3(0,1,0)), hash(i + vec3(1,1,0)), f.x), f.y),
                  mix(mix( hash(i + vec3(0,0,1)), hash(i + vec3(1,0,1)), f.x),
                      mix( hash(i + vec3(0,1,1)), hash(i + vec3(1,1,1)), f.x), f.y), f.z);
    return n;
}

float fbm(vec3 p) {
    float a = 0.5;
    float f = 0.0;
    for (int i = 0; i < 5; i++) {
        f += a * noise(p);
        p *= 2.0;
        a *= 0.5;
    }
    return f;
}

//----------------------
// Life-Fluid Density Field
//----------------------

float lifeFluidField(vec3 p, vec3 mpos, float t) {
    // Organism pulsation
    float lifePulse = sin(dot(p, vec3(0.8, 1.3, 0.5)) * 3.0 - t * 3.5) * pulse;

    // Fluidic motion displacement
    vec3 fluidMotion = vec3(
        sin(t * 0.4 + p.y * 2.0),
        sin(t * 0.5 + p.z * 1.5),
        cos(t * 0.3 + p.x * 2.2)
    ) * flow;

    float fluid = fbm(p + fluidMotion);

    // Decay & regrowth cycles
    float decay = sin(length(p) * 3.0 - t * 2.0) * 0.3;

    // Local mouse interaction
    float mfield = exp(-length(p - mpos) * 6.0);

    return fluid + lifePulse + decay + mfield;
}

//----------------------
// Spectral Color
//----------------------

vec3 organismColor(float d, vec3 p, float t) {
    float e = clamp(d, 0.0, 1.5);
    float hueShift = sin(t * colorSpeed + dot(p, vec3(1.2, 0.8, 1.0)));
    vec3 base = 0.5 + 0.5 * cos(6.2831853 * (vec3(0.15, 0.4, 0.65) + hueShift + e * 1.4));
    
    // Soft pulse bloom
    base += 0.2 * sin(dot(p, vec3(5.0, 3.0, 2.0)) + t * 4.0);

    return clamp(base * e * colorBoost, 0.0, 1.5);
}

//----------------------
// Raymarching
//----------------------

vec4 raymarch(vec3 ro, vec3 rd, vec3 mpos) {
    float t = 0.0;
    vec3 col = vec3(0.0);
    float opacity = 0.0;

    for (int i = 0; i < 128; i++) {
        vec3 pos = ro + rd * t;
        float d = lifeFluidField(pos, mpos, TIME * speed);
        float alpha = smoothstep(0.3, 1.2, d) * density;

        vec3 c = organismColor(d, pos, TIME * speed);
        c *= alpha;

        col += (1.0 - opacity) * c;
        opacity += alpha * (1.0 - opacity);

        t += 0.03;
        if (opacity > 0.98 || t > 6.0) break;
    }

    return vec4(pow(col, vec3(0.85)), 1.0);
}

//----------------------
// Camera
//----------------------

mat3 camLook(vec3 ro, vec3 ta){
    vec3 z = normalize(ta - ro);
    vec3 x = normalize(cross(vec3(0,1,0), z));
    vec3 y = cross(z, x);
    return mat3(x, y, z);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord - RENDERSIZE) / RENDERSIZE.y;

    vec2 m = ((RENDERSIZE * 0.5) / RENDERSIZE) * 2.0 - 1.0;
    m.x *= RENDERSIZE.x / RENDERSIZE.y;
    vec3 mpos = vec3(m * 1.5, 0.0);

    vec3 ro = vec3(0.0, 0.0, -4.0);
    vec3 ta = vec3(0.0);
    mat3 cam = camLook(ro, ta);
    vec3 rd = normalize(cam * vec3(uv, 1.6));

    vec4 col = raymarch(ro, rd, mpos);
    fragColor = col;
}

// --- ISF entry point: bridge to Shadertoy mainImage ---
void main() {
    mainImage(gl_FragColor, gl_FragCoord.xy);
}
