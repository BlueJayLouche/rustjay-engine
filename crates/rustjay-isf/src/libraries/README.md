# Vendored GLSL libraries

Shaders written for MadMapper `#include` these. They are vendored verbatim,
license notice and all, from [madmappersoftware/MadMapper-Materials][repo]
(Apache-2.0) — MadMapper publishes them itself, so there is nothing to
reimplement and nothing to clean-room.

To add another (`MadNoise.glsl`, `MadSDF.glsl`), drop the file in beside this
one and add a line to `LIBRARIES` in `../compile.rs`.

[repo]: https://github.com/madmappersoftware/MadMapper-Materials/tree/main/Libraries
