/*{
	"DESCRIPTION": "test: passthrough of inputImage via IMG_THIS_PIXEL",
	"ISFVSN": "2.0",
	"INPUTS": [
		{
			"NAME": "inputImage",
			"TYPE": "image"
		}
	]
}*/

void main()
{
	gl_FragColor = IMG_THIS_PIXEL(inputImage);
}
