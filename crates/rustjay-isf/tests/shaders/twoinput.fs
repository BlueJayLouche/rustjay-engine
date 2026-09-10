/*{
	"DESCRIPTION": "test: mixes two image inputs by progress — the second declared image input must bind to the host's second texture, not to the first",
	"ISFVSN": "2.0",
	"CATEGORIES": ["Transition"],
	"INPUTS": [
		{
			"NAME": "progress",
			"TYPE": "float",
			"DEFAULT": 0.0,
			"MIN": 0.0,
			"MAX": 1.0
		},
		{
			"NAME": "startImage",
			"TYPE": "image"
		},
		{
			"NAME": "endImage",
			"TYPE": "image"
		}
	]
}*/

void main()
{
	gl_FragColor = mix(IMG_THIS_PIXEL(startImage), IMG_THIS_PIXEL(endImage), progress);
}
