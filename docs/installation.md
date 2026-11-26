# Installation

Linux x86-64 packages, a portable tar archive, and a `SHA256SUMS` checksum file are published on the [latest release](https://github.com/ryanpetris/softsink/releases/latest) page.

## Distribution Packages

Native packages target Linux x86-64 systems:

- **Arch Linux**: built against the Ubuntu 26.04 baseline (glibc 2.43).
- **Ubuntu 26.04 (`resolute`)**: uses the `resolute` `.deb` package.
- **Debian 13 (`trixie`)**: compiled natively on Debian 13 with Debian 13 libraries (`trixie` `.deb` package).

Install on Arch Linux:

```sh
sudo pacman -U softsink-*-x86_64.pkg.tar.zst
```

Install on Ubuntu 26.04 (substitute `trixie` for Debian 13):

```sh
sudo apt install ./softsink_*resolute_amd64.deb
```

Distribution packages install `softsink` on your `PATH`, add an application menu entry and icons, install documentation and third-party notices, and pull in runtime library dependencies. To upgrade, download the newer package and run the same command; saved settings are preserved.

## Portable Tar Archive

`softsink-<version>-linux-amd64.tar.gz` contains the `softsink` executable (Ubuntu 26.04 glibc 2.43 baseline), desktop entry, icons, documentation, checksums, and third-party notices.

Extract the archive, ensure the [runtime requirements](#runtime-requirements) are installed on your distribution, and run `./softsink` from the extracted directory.

## Runtime Requirements

softsink runs on Linux only and requires:

- **Graphics and Wayland**: The Vulkan loader and a Vulkan driver whose Wayland surface supports mailbox presentation, opaque and pre-multiplied composite alpha, and `Bgra8Unorm` or `Rgba8Unorm`.
- **Client libraries**: Wayland (`libwayland-client`), `libxkbcommon`, PipeWire (`libpipewire-0.3`), and ALSA (`libasound`).
- **Audio passthrough**: A running PipeWire server with its ALSA plugin (`pipewire-audio` on Arch Linux) and WirePlumber to expose ALSA devices. This applies to both distribution packages and the portable archive.

## Launching

Open **softsink** from your application menu or run `softsink` from a terminal. The only command-line option is `--version`, which prints the build version.

See [Using softsink](usage.md) for controls, capture modes, scaling, audio passthrough, and the stats overlay.
