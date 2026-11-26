#!/usr/bin/env bash
# Install and remove the package in a fresh runtime container.
set -euo pipefail
kind=$1
version=$SOFTSINK_VERSION
unset DISPLAY WAYLAND_DISPLAY

require_absent() {
  if "$@" >/dev/null 2>&1; then
    echo 'Expected softsink to be absent from the package database' >&2
    exit 1
  fi
}

case "$kind" in
  arch)
    require_absent pacman -Qq softsink
    pacman -Syu --noconfirm --needed python vulkan-swrast
    # The base image skips package documentation to save space.
    sed -i '/^NoExtract.*usr\/share\/doc\/\*/a NoExtract = !usr/share/doc/softsink/*' /etc/pacman.conf
    pacman -U --noconfirm /input.pkg.tar.zst
    python3 tools/check_linux_package.py "$kind" "$version"
    pacman -R --noconfirm softsink
    require_absent pacman -Qq softsink
    ;;
  trixie|resolute)
    require_absent dpkg-query -W softsink
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    # The Ubuntu base image filters package documentation.
    apt-get install -y --no-install-recommends \
      -o 'DPkg::Options::=--path-include=/usr/share/doc/softsink/*' python3 /input.deb
    python3 tools/check_linux_package.py "$kind" "$version"
    apt-get purge -y softsink
    require_absent dpkg-query -W softsink
    ;;
  tar)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends python3 libvulkan1 mesa-vulkan-drivers \
      libwayland-client0 libxkbcommon0 libpipewire-0.3-0t64 libasound2t64
    python3 tools/check_release_archive.py /input.tar.gz "$version"
    work=$(mktemp -d)
    tar -xzf /input.tar.gz -C "$work" --strip-components=1
    (cd "$work" && python3 /src/tools/check_linux_package.py "$kind" "$version")
    rm -r "$work"
    test ! -e "$work"
    exit
    ;;
  *)
    echo "Unknown package type: $kind" >&2
    exit 1
    ;;
esac

for path in /usr/bin/softsink /usr/share/doc/softsink /usr/share/licenses/softsink \
    /usr/share/applications/dev.petris.softsink.desktop \
    /usr/share/icons/hicolor/512x512/apps/dev.petris.softsink.png \
    /usr/share/icons/hicolor/scalable/apps/dev.petris.softsink.svg; do
  if [ -e "$path" ]; then
    echo "Package removal left $path" >&2
    exit 1
  fi
done
