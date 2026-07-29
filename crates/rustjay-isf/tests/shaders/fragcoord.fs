/*{
	"DESCRIPTION": "test: outputs gl_FragCoord / RENDERSIZE as RG",
	"ISFVSN": "2.0",
	"INPUTS": []
}*/

void main()
{
	gl_FragColor = vec4(gl_FragCoord.xy / RENDERSIZE, 0.0, 1.0);
}
