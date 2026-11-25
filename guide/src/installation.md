# Installation

## Prerequisites

- **Rust 1.80+** — install via [rustup](https://rustup.rs/)
- **cargo** (comes with Rust)
- A C/C++ build toolchain (for linking wgpu's native backends):
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Windows: Visual Studio Build Tools (MSVC)
  - Linux: `gcc`, `pkg-config`, and dev headers (`build-essential`, `libssl-dev`, etc.)

## Platform requirements

### macOS

rustjay-engine renders via **Metal**. No extra GPU drivers needed.

Syphon video sharing needs no install: the `syphon-core` crate ships a universal
(arm64 + x86_64) Syphon.framework and the build scripts point the binary at it.

One thing to watch for on an Apple Silicon Mac. A copy of Syphon.framework in
`/Library/Frameworks/` — left by an old VJ app installer, say — is also on the
runtime search path, and copies from before the Apple Silicon transition are
x86_64-only. dyld picks the *first* match rather than the first *loadable* one,
so a stale system copy that comes earlier in the path aborts the launch:

```
dyld[…]: Library not loaded: @rpath/Syphon.framework/Versions/A/Syphon
  Reason: … (mach-o file, but is an incompatible architecture (have 'x86_64', need 'arm64'))
```

The bundled framework is searched ahead of `/Library/Frameworks`, so this only
bites if that ordering is lost. Check what you have with:

```sh
# Should list arm64 (a bare `x86_64` is the stale case above)
lipo -archs /Library/Frameworks/Syphon.framework/Versions/A/Syphon
```

Fix it by replacing that copy with a [current
release](https://github.com/Syphon/Syphon-Framework/releases), or removing it and
letting the bundled one be found. To force a specific copy, point
`SYPHON_FRAMEWORK_DIR` at the directory *containing* `Syphon.framework` and
rebuild.

### Windows

rustjay-engine renders via **Vulkan** or **DX12**. Ensure your GPU drivers are up to date.

Spout video sharing uses DirectX interop and is included automatically — no separate install needed.

### Linux

rustjay-engine renders via **Vulkan**. Install the Vulkan SDK for your distribution:

```sh
# Ubuntu / Debian
sudo apt install vulkan-tools libvulkan-dev

# Arch
sudo pacman -S vulkan-tools vulkan-icd-loader
```

V4L2 loopback output requires the `v4l2loopback` kernel module:

```sh
sudo modprobe v4l2loopback
```

## NDI (optional)

NDI video-over-IP is behind the `ndi` feature, which is off by default:

```toml
[dependencies]
rustjay-engine = { version = "0.2", features = ["ndi"] }
```

Building needs nothing NDI-specific. At run time the app loads the NDI 6 runtime
(from [NDI Tools](https://ndi.video/tools/) or the [NDI SDK](https://ndi.video/download-ndi-sdk/)),
looking next to the executable first, then in `NDI_RUNTIME_DIR_V6`, then on the
platform's library search path. To ship it, put `libndi.dylib` (macOS, in
`Contents/Frameworks`), `libndi.so.6` (Linux) or `Processing.NDI.Lib.x64.dll`
(Windows) beside the binary. Without a runtime, NDI sources simply don't appear.

## Ableton Link (optional)

The `link` feature builds Ableton Link's native C++ via CMake, which this repo
configures to use the **Ninja** generator (see `.cargo/config.toml`). You need
both **CMake ≥ 3.14** and **Ninja** installed:

```sh
# macOS
brew install cmake ninja

# Ubuntu (the Ninja binary ships in the "ninja-build" package)
sudo apt install cmake ninja-build

# Fedora
sudo dnf install cmake ninja-build

# Arch
sudo pacman -S cmake ninja

# Windows
winget install Kitware.CMake Ninja-build.Ninja
# (or download CMake from https://cmake.org/download/ and Ninja from
#  https://github.com/ninja-build/ninja/releases)
```

If Ninja is missing you'll see `CMake was unable to find a build program
corresponding to "Ninja"` when building the `link` feature.

Enabling the `link` feature links against Ableton Link, which is **GPL-2.0+**. This changes the license of your resulting binary.

## Creating a new project

```sh
cargo new my-effect
cd my-effect
```

Add rustjay-engine to `Cargo.toml`:

```toml
[dependencies]
rustjay-engine = "0.1"
bytemuck = { version = "1.21", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
env_logger = "0.11"
log = "0.4"
```

The engine repo carries two runtime fixes that crates.io builds don't get: `wgpu-hal`
(EGL config selection on Raspberry Pi / Mesa vc4) and `nokhwa-bindings-macos` (USB and
Continuity cameras that crash AVFoundation). If you target either, pull them in:

```toml
[patch.crates-io]
wgpu-hal              = { git = "https://github.com/BlueJayLouche/rustjay-engine" }
nokhwa-bindings-macos = { git = "https://github.com/BlueJayLouche/rustjay-engine" }
```

Build it once to pull dependencies (this takes a few minutes the first time):

```sh
cargo build
```

You're ready. Head to [Your First Effect](getting-started/README.md).
