/*{
	"CREDIT": "rustjay-engine (ShaderGlass example)",
	"ISFVSN": "2",
	"CATEGORIES": [
		"Retro", "CRT", "Filter"
	],
	"INPUTS": [
		{
			"NAME": "inputImage",
			"TYPE": "image"
		},
		{
			"NAME": "curvature",
			"LABEL": "Screen Curvature",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 0.5,
			"DEFAULT": 0.15
		},
		{
			"NAME": "scanlineIntensity",
			"LABEL": "Scanline Intensity",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 1.0,
			"DEFAULT": 0.5
		},
		{
			"NAME": "scanlineCount",
			"LABEL": "Scanline Count",
			"TYPE": "float",
			"MIN": 100.0,
			"MAX": 1080.0,
			"DEFAULT": 240.0
		},
		{
			"NAME": "maskIntensity",
			"LABEL": "Aperture Mask",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 1.0,
			"DEFAULT": 0.3
		},
		{
			"NAME": "brightness",
			"LABEL": "Brightness Boost",
			"TYPE": "float",
			"MIN": 0.5,
			"MAX": 2.5,
			"DEFAULT": 1.3
		},
		{
			"NAME": "vignette",
			"LABEL": "Vignette",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 1.0,
			"DEFAULT": 0.3
		},
		{
			"NAME": "correctAspect",
			"LABEL": "Correct Aspect Ratio",
			"TYPE": "bool",
			"DEFAULT": true
		},
		{
			"NAME": "sourceAspect",
			"LABEL": "Source Aspect (auto)",
			"TYPE": "float",
			"MIN": 0.1,
			"MAX": 4.0,
			"DEFAULT": 1.7777
		}
	]
}*/

// CRT monitor simulation as an input filter: barrel-warps the source frame,
// applies an aperture-grille RGB mask, horizontal scanlines, a vignette and a
// brightness boost to compensate for the darkening the mask/scanlines cause.

// Barrel distortion around screen centre. `amt` is the curvature strength.
vec2 curveUV(vec2 uv, float amt) {
	uv = uv * 2.0 - 1.0;            // -> [-1, 1]
	vec2 offset = abs(uv.yx) / vec2(6.0, 4.0);
	uv = uv + uv * offset * offset * amt * 4.0;
	return uv * 0.5 + 0.5;          // -> [0, 1]
}

void main() {
	vec2 uv = isf_FragNormCoord;

	// Aspect-ratio correction: letterbox/pillarbox the source so it keeps its
	// native proportions inside the output. `sourceAspect` is fed by the host
	// (the example's Shader tab) from the live input resolution.
	if (correctAspect) {
		float outAspect = RENDERSIZE.x / RENDERSIZE.y;
		vec2 s = vec2(1.0);
		if (sourceAspect > outAspect) {
			s.y = outAspect / sourceAspect;   // bars top/bottom
		} else {
			s.x = sourceAspect / outAspect;   // bars left/right
		}
		uv = (uv - 0.5) / s + 0.5;
		if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
			gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0);
			return;
		}
	}

	uv = curveUV(uv, curvature);

	// Outside the (curved) tube = black bezel.
	if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
		gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0);
		return;
	}

	vec3 col = IMG_NORM_PIXEL(inputImage, uv).rgb;

	// Aperture-grille mask: tint successive screen columns R/G/B.
	float colIdx = mod(gl_FragCoord.x, 3.0);
	vec3 mask = vec3(1.0);
	if (colIdx < 1.0)      mask = vec3(1.0, 0.7, 0.7);
	else if (colIdx < 2.0) mask = vec3(0.7, 1.0, 0.7);
	else                   mask = vec3(0.7, 0.7, 1.0);
	col *= mix(vec3(1.0), mask, maskIntensity);

	// Horizontal scanlines at a fixed line count (e.g. 240 for a 240p look),
	// independent of output resolution.
	float scan = sin(uv.y * scanlineCount * 3.14159265) * 0.5 + 0.5;
	col *= 1.0 - scanlineIntensity * (1.0 - scan);

	// Brightness compensation.
	col *= brightness;

	// Vignette toward the corners.
	vec2 vc = uv * 2.0 - 1.0;
	float vig = 1.0 - dot(vc, vc) * vignette * 0.5;
	col *= clamp(vig, 0.0, 1.0);

	gl_FragColor = vec4(col, 1.0);
}
