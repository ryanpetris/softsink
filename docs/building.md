# Building softsink

This guide covers building softsink locally, versioning release builds, and building and validating Linux packages in containers.

## Building Locally

### Prerequisites

- Rust 1.95 or newer (release containers use Rust 1.99.1 via `rustup`).
- A C compiler, `make`, `pkg-config`, `cmake`, and `nasm`. By default, `turbojpeg-sys` compiles and statically links a bundled `libjpeg-turbo`. Set `TURBOJPEG_SOURCE=pkg-config` to link a system `libjpeg-turbo` 3.0 or newer instead (which requires neither `cmake` nor `nasm`).
- `libclang` for generating C bindings, and Linux kernel headers providing `linux/videodev2.h`.
- PipeWire and ALSA development files.

Debian and Ubuntu:

```sh
sudo apt-get install build-essential cmake libasound2-dev libclang-dev \
  libpipewire-0.3-dev linux-libc-dev nasm pkg-config
```

Arch Linux:

```sh
sudo pacman -S base-devel clang cmake nasm pkgconf libpipewire alsa-lib
```

### Build and Run

Build and run from the repository root:

```sh
cargo run --release
```

Build a locked release binary or run unit tests:

```sh
cargo build --locked --release
cargo test --locked
```

## Versioning

Tracked manifests use `0.0.0`. Set `SOFTSINK_VERSION` to `MAJOR.MINOR.PATCH` (no `v` prefix or leading zeroes) to stamp a release version into the binary and packages. Optional variables:

- `SOURCE_DATE_EPOCH`: Unix timestamp in seconds for deterministic package timestamps.
- `SOFTSINK_HOMEPAGE`: Package homepage URL override (defaults to `https://github.com/ryanpetris/softsink`).

The GitHub Actions release workflow (`.github/workflows/release.yml`) triggers on `vMAJOR.MINOR.PATCH` tags, strips the leading `v` for `SOFTSINK_VERSION`, sets `SOURCE_DATE_EPOCH` from the tagged commit, builds and validates all packages, and publishes them alongside `SHA256SUMS`.

## Packaging with Containers

Packaging recipes run on Linux x86-64 inside containers built from `docker/` and write artifacts to `build/release/`.

### Prerequisites

- Bash, Python 3, and `just`
- [Docker](https://docs.docker.com/engine/install/) (or Podman via `DOCKER=podman`)

Debian and Ubuntu:

```sh
sudo apt-get install just python3
```

Arch Linux:

```sh
sudo pacman -S just python
```

### Recipes

Run all recipes from the repository root (run `just` with no arguments to list available recipes):

```sh
export SOFTSINK_VERSION=1.2.3
just check-package-host-tar
export HOST_ARCHIVE="build/release/softsink-$SOFTSINK_VERSION-linux-amd64.tar.gz"
just check-package-arch
just DEB_DISTRIBUTION=resolute check-package-deb
unset HOST_ARCHIVE
just DEB_DISTRIBUTION=trixie check-package-deb
```

Use `package-host-tar`, `package-host-arch`, and `package-host-deb` to build packages without running the verification containers.

Target details:

- **Portable tar archive** (`package-host-tar`): Builds on Ubuntu 26.04 (glibc 2.43) and writes `build/release/softsink-<version>-linux-amd64.tar.gz`, including the binary, desktop entry, icons, docs, checksums, and generated third-party notices.
- **Arch Linux** (`package-host-arch`): Repackages the portable archive into `build/release/softsink-<version>-1-x86_64.pkg.tar.zst`. Accepts `HOST_ARCHIVE`.
- **Ubuntu 26.04** (`DEB_DISTRIBUTION=resolute package-host-deb`): Repackages the portable archive into `build/release/softsink_<version>-1~resolute_amd64.deb`. Accepts `HOST_ARCHIVE`.
- **Debian 13** (`DEB_DISTRIBUTION=trixie package-host-deb`): Compiles its own executable against Debian 13 libraries into `build/release/softsink_<version>-1~trixie_amd64.deb` and rejects `HOST_ARCHIVE`.

### Package Validation

Each `check-package-*` recipe installs or extracts the built artifact in a fresh disposable runtime container without a GPU or display and verifies:

- Exact file set, permissions, and desktop entry metadata.
- Dependency inventory (`dependencies.json`), all bundled component and crate notices, and every notice's SHA-256 hash.
- Linux x86-64 ELF target, expected dynamic library linkage (no missing libraries and no dynamic `libturbojpeg`), and loadable runtime shared libraries.
- `softsink --version` output matching `SOFTSINK_VERSION`.
- Clean removal leaving no packaged files behind.

## Repository Layout

- `src/`: Rust application source and WGSL shaders (`capture`, `decode`, `render`, `scale`, `audio`, `ui`, `settings`, `inhibit`).
- `assets/`: Bundled Inter font and license text.
- `docs/`: Installation, usage, and build documentation.
- `docker/`: Dockerfiles for host builds and packaging, plus the `just` installer used by the release workflow.
- `packaging/`: Arch (`PKGBUILD`), Debian/Ubuntu packaging files, desktop entry, icons, container build/check scripts, and fallback third-party notices.
- `tools/`: Version validation, third-party notice generation, release archive verification, and Linux package check scripts.
