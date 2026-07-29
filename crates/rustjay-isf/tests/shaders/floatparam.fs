/*{
	"DESCRIPTION": "test: outputs a float input as grey",
	"ISFVSN": "2.0",
	"INPUTS": [
		{
			"NAME": "v",
			"TYPE": "float",
			"MIN": 0.0,
			"MAX": 1.0,
			"DEFAULT": 0.25
		}
	]
}*/

void main()
{
	gl_FragColor = vec4(vec3(v), 1.0);
}
