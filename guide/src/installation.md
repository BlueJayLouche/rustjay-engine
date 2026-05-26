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

For Syphon video sharing, install [Syphon.framework](https://github.com/Syphon/Syphon-Framework/releases) into `/Library/Frameworks/`. Many VJ apps (Resolume, VDMX, MadMapper) bundle it — if you've installed any of those, Syphon is already present.

```sh
# Verify the framework is installed
ls /Library/Frameworks/Syphon.framework
```

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

NDI video-over-IP requires the [NDI SDK](https://ndi.video/download-ndi-sdk/) to be installed. The `ndi` feature is enabled by default — if you don't have the SDK installed and see linker errors, disable it:

```toml
[dependencies]
rustjay-engine = { git = "...", default-features = false }
```

## Ableton Link (optional)

The `link` feature requires **CMake ≥ 3.14**:

```sh
# macOS
brew install cmake

# Ubuntu
sudo apt install cmake

# Windows — download from https://cmake.org/download/
```

Enabling the `link` feature links against Ableton Link, which is **GPL-2.0+**. This changes the license of your resulting binary.

## Creating a new project

```sh
cargo new my-effect
cd my-effect
```

Add rustjay-engine to `Cargo.toml`:

```toml
[dependencies]
rustjay-engine = { git = "https://github.com/BlueJayLouche/rustjay-engine" }
bytemuck = { version = "1.21", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
env_logger = "0.11"
log = "0.4"
```

Build it once to pull dependencies (this takes a few minutes the first time):

```sh
cargo build
```

You're ready. Head to [Your First Effect](getting-started/README.md).
