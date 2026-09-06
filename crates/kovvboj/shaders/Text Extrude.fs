/*{
    "DESCRIPTION": "Fakes depth on anything with an alpha edge — text especially — by stacking offset copies behind the face. Cheap, and it reads as 3D.",
    "CREDIT": "kovvboj",
    "CATEGORIES": ["Stylize"],
    "INPUTS": [
        { "NAME": "inputImage", "TYPE": "image" },
        { "NAME": "depth", "TYPE": "float", "MIN": 0.0, "MAX": 0.5, "DEFAULT": 0.08 },
        { "NAME": "angle", "TYPE": "float", "MIN": -180.0, "MAX": 180.0, "DEFAULT": 135.0 },
        { "NAME": "steps", "TYPE": "long", "MIN": 1, "MAX": 64, "DEFAULT": 32 },
        { "NAME": "sideColor", "TYPE": "color", "DEFAULT": [0.35, 0.35, 0.4, 1.0] },
        { "NAME": "falloff", "TYPE": "float", "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.6 }
    ]
}*/

// The extrusion runs from the far end back towards the face, so nearer slices
// paint over further ones and the side ends up shaded along its length.
void main() {
    vec2 uv = isf_FragNormCoord;
    // Keep the angle true whatever the output's aspect: the offset is measured
    // in height units and squeezed back into normalised x.
    float aspect = RENDERSIZE.y / RENDERSIZE.x;
    float a = radians(angle);
    vec2 dir = vec2(cos(a) * aspect, sin(a));

    int n = max(steps, 1);
    vec4 side = vec4(0.0);
    for (int i = 64; i >= 1; i--) {
        if (i > n) continue;
        float t = float(i) / float(n);
        vec4 s = IMG_NORM_PIXEL(inputImage, uv - dir * depth * t);
        if (s.a <= 0.001) continue;
        // Darker towards the far end, so the extrusion has a direction.
        float shade = mix(1.0, 1.0 - falloff, t);
        side = vec4(sideColor.rgb * shade, sideColor.a * s.a);
    }

    vec4 face = IMG_THIS_NORM_PIXEL(inputImage);
    gl_FragColor = mix(side, face, face.a);
}
