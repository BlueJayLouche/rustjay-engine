# Vendored GLSL libraries

Shaders written for MadMapper `#include` these. They are vendored verbatim,
license notice and all, from [madmappersoftware/MadMapper-Materials][repo]
(Apache-2.0) — MadMapper publishes them itself, so there is nothing to
reimplement and nothing to clean-room.

To add another, drop the file in beside this one and add a line to `LIBRARIES`
in `../compile.rs`.

Not vendored: `auto_all.glsl`. That one is not a library — MadMapper generates a
copy per material and ships it inside the material's own folder, and it carries
its own ISF header declaring inputs. Supporting it means resolving includes
relative to the shader's directory and merging headers; see `LASER_MATERIALS.md`.

[repo]: https://github.com/madmappersoftware/MadMapper-Materials/tree/main/Libraries
