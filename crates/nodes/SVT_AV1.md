<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# SVT-AV1 for StreamKit

The `video::svt_av1::encoder` node requires **libsvtav1enc ≥ 4.0**.

## Static build (recommended)

The easiest way to enable SVT-AV1 is the `svt_av1_static` feature, which
downloads a pinned SVT-AV1 release at `cargo build` time and links it
statically.  No system-wide installation or `sudo` is needed.

### Prerequisites

- `cmake` (≥ 3.16)
- A C compiler (`gcc` / `clang`)
- `nasm` (optional — enables x86-64 SIMD assembly; without it CMake falls
  back to C-only, which still works but is slower)
- `curl` (for downloading the source tarball)

On Debian/Ubuntu:

```bash
sudo apt install cmake nasm build-essential curl
```

### Build & run

```bash
# Via just (recommended)
just extra_features="--features svt_av1_static" skit

# Or build in release mode
just extra_features="--features svt_av1_static" build-skit

# Or directly with cargo
cargo run -p streamkit-server --features "moq,svt_av1_static"
```

### Using a pre-downloaded source tree

If you already have the SVT-AV1 source, point the build at it to skip the
download step:

```bash
SVT_AV1_SRC_DIR=/path/to/SVT-AV1 cargo build --features svt_av1_static
```

The directory must contain `CMakeLists.txt` at its root.

## System library (alternative)

If you prefer to link against a system-installed shared library, use the
`svt_av1` feature (without `_static`).  This requires **libsvtav1enc ≥ 4.0**
installed and discoverable via `pkg-config`.

Ubuntu/Debian distro packages ship very old versions (e.g. Ubuntu 22.04
ships ~0.9.0), so **building from source** is required.

### Quick install (build from source)

```bash
# 1. Remove any old distro packages
sudo apt remove --purge -y svt-av1 libsvtav1-dev
sudo apt autoremove -y

# 2. Install build dependencies
sudo apt update
sudo apt install -y \
  git build-essential cmake ninja-build nasm yasm pkg-config

# 3. Build and install the latest release
cd /tmp
git clone https://gitlab.com/AOMediaCodec/SVT-AV1.git
cd SVT-AV1
git checkout v4.1.0

cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr/local

cmake --build build -j"$(nproc)"
sudo cmake --install build
sudo ldconfig

# 4. Verify
SvtAv1EncApp --version
pkg-config --modversion SvtAv1Enc
ldconfig -p | grep -i SvtAv1
```

### Build & run

```bash
# Via just (recommended)
just extra_features="--features svt_av1" skit

# Or build in release mode
just extra_features="--features svt_av1" build-skit

# Or directly with cargo
cargo run -p streamkit-server --features "moq,svt_av1"
```

If you installed to a non-standard prefix, set `PKG_CONFIG_PATH`:

```bash
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig:$PKG_CONFIG_PATH
export LD_LIBRARY_PATH=/usr/local/lib:$LD_LIBRARY_PATH
```

## Troubleshooting

### LTO build errors

If LTO causes issues during the SVT-AV1 build, disable it:

```bash
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr/local \
  -DSVT_AV1_LTO=OFF
```

### Checking for leftover installations

```bash
dpkg -l | grep -i svt
ldconfig -p | grep -i svt
ls -l /usr/local/lib/libSvtAv1*
ls -l /usr/local/include/svt-av1
ls -l /usr/local/lib/pkgconfig | grep -i svt
```

### FFmpeg integration

If you use FFmpeg with `libsvtav1`, you may need to rebuild FFmpeg
against the new library after upgrading SVT-AV1 (ABI changed in 4.x):

```bash
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig:$PKG_CONFIG_PATH
export LD_LIBRARY_PATH=/usr/local/lib:$LD_LIBRARY_PATH
./configure --enable-libsvtav1
```

### User-local install (no sudo)

To install without root privileges:

```bash
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=$HOME/opt/svt-av1-4.1.0

cmake --build build -j"$(nproc)"
cmake --install build

# Add to your shell profile
export PKG_CONFIG_PATH=$HOME/opt/svt-av1-4.1.0/lib/pkgconfig:$PKG_CONFIG_PATH
export LD_LIBRARY_PATH=$HOME/opt/svt-av1-4.1.0/lib:$LD_LIBRARY_PATH
```
