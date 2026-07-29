/*{
	"DESCRIPTION": "test: Shadertoy-style bare mainImage entry (bridge synthesis)",
	"ISFVSN": "2.0",
	"INPUTS": []
}*/

void mainImage(out vec4 fragColor, in vec2 fragCoord)
{
	fragColor = vec4(fragCoord / RENDERSIZE, 0.0, 1.0);
}
