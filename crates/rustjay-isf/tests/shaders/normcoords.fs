/*{
	"DESCRIPTION": "test: outputs isf_FragNormCoord as RG",
	"ISFVSN": "2.0",
	"INPUTS": []
}*/

void main()
{
	gl_FragColor = vec4(isf_FragNormCoord, 0.0, 1.0);
}
