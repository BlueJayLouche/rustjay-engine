/*{
	"DESCRIPTION": "test: MadMapper material entry point + a time_base generator",
	"CREDIT": "rustjay-isf tests",
	"INPUTS": [
		{ "NAME": "mat_speed", "TYPE": "float", "MIN": 0., "MAX": 4., "DEFAULT": 1. }
	],
	"GENERATORS": [
		{ "NAME": "mat_pos", "TYPE": "time_base", "PARAMS": { "speed": "mat_speed" } }
	]
}*/

vec4 materialColorForPixel(vec2 texCoord)
{
	return vec4(texCoord, fract(mat_pos), 1.0);
}
