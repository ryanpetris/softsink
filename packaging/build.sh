#!/usr/bin/env bash
# Write the package to stdout for export from the build container.
set -euo pipefail
exec 3>&1 >&2

export SOFTSINK_VERSION
SOFTSINK_VERSION=$(python3 tools/version.py)
name="softsink-$SOFTSINK_VERSION-linux-amd64"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$1" in
  tar)
    if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then unset SOURCE_DATE_EPOCH; fi
    export CARGO_TARGET_DIR="$work/target"
    cargo build --locked --release --target x86_64-unknown-linux-gnu
    python3 tools/package_host.py "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/softsink" "$work"
    file="$work/$name.tar.gz"
    ;;
  arch|deb)
    epoch=$(python3 tools/check_release_archive.py /input.tar.gz "$SOFTSINK_VERSION")
    export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$epoch}"
    if [ "$1" = arch ]; then
      cp packaging/arch/PKGBUILD "$work/"
      cp /input.tar.gz "$work/$name.tar.gz"
      export SOFTSINK_ARCHIVE_SHA256
      SOFTSINK_ARCHIVE_SHA256=$(sha256sum /input.tar.gz | cut -d ' ' -f 1)
      chown -R builder:builder "$work"
      cd "$work"
      runuser -u builder -- makepkg --nodeps --nosign PKGEXT=.pkg.tar.zst
      file="$work/softsink-$SOFTSINK_VERSION-1-x86_64.pkg.tar.zst"
    else
      distribution=$2
      mkdir "$work/source"
      tar -xzf /input.tar.gz -C "$work/source" --strip-components=1 --no-same-owner
      cp -R packaging/debian "$work/source/debian"
      cd "$work/source"
      cp LICENSE debian/copyright
      stamp=$(LC_ALL=C date -u -R -d "@$SOURCE_DATE_EPOCH")
      sed -i -e "s/@VERSION@/$SOFTSINK_VERSION-1~$distribution/" \
        -e "s/@DISTRIBUTION@/$distribution/" -e "s/@DATE@/$stamp/" debian/changelog
      dpkg-buildpackage --build=binary --no-sign
      file="$work/softsink_${SOFTSINK_VERSION}-1~${distribution}_amd64.deb"
    fi
    ;;
  *)
    echo "Unknown package type: $1" >&2
    exit 1
    ;;
esac

cat "$file" >&3
