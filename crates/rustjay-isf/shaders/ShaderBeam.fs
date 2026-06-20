/*{
	"CREDIT": "rustjay-engine (ShaderGlass example) — after Blur Busters' CRT Beam Simulator",
	"ISFVSN": "2",
	"CATEGORIES": [
		"Retro", "CRT", "Motion"
	],
	"INPUTS": [
		{
			"NAME": "inputImage",
			"TYPE": "image"
		},
		{
			"NAME": "speed",
			"LABEL": "Roll Speed (Hz)",
			"TYPE": "float",
			"MIN": 1.0,
			"MAX": 30.0,
			"DEFAULT": 8.0
		},
		{
			"NAME": "decay",
			"LABEL": "Phosphor Decay",
			"TYPE": "float",
			"MIN": 0.02,
			"MAX": 1.0,
			"DEFAULT": 0.2
		},
		{
			"NAME": "bfiStrength",
			"LABEL": "Beam / BFI Strength",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 1.0,
			"DEFAULT": 1.0
		},
		{
			"NAME": "beamBrightness",
			"LABEL": "Brightness Boost",
			"TYPE": "float",
			"MIN": 0.5,
			"MAX": 4.0,
			"DEFAULT": 2.0
		}
	]
}*/

// CRT Beam Simulator + Black Frame Insertion, after Blur Busters / ShaderBeam.
// A bright horizontal "scan beam" rolls down the screen; each row is brightest
// just after the beam passes it, then decays (phosphor). Outside the beam the
// image is darkened (BFI) — the higher the display refresh vs content, the more
// this reduces sample-and-hold motion blur. Intended as a chain FX after a CRT
// filter, on a high-refresh display.

void main() {
	vec3 col = IMG_THIS_PIXEL(inputImage).rgb;

	// Beam centre rolls 0→1 down the screen `speed` times per second.
	float beam = fract(TIME * speed);

	// Time since the beam passed this row (0 = just lit), wrapped per sweep.
	float t = fract(beam - isf_FragNormCoord.y);

	// Phosphor decay behind the beam.
	float lit = exp(-t / max(decay, 0.001));

	// Blend between the untouched image and the beam-windowed image.
	float win = mix(1.0, lit, bfiStrength);

	col *= win * beamBrightness;
	gl_FragColor = vec4(col, 1.0);
}
