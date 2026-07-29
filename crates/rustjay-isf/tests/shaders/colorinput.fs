/*{
	"DESCRIPTION": "test: outputs a color input with a non-black default",
	"ISFVSN": "2.0",
	"INPUTS": [
		{
			"NAME": "tint",
			"TYPE": "color",
			"DEFAULT": [ 1.0, 0.0, 0.5, 1.0 ]
		}
	]
}*/

void main()
{
	gl_FragColor = tint;
}
