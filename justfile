set shell := ['bash', '-eu', '-o', 'pipefail', '-c']

export DOCKER := env('DOCKER', 'docker')
export SOFTSINK_VERSION := env('SOFTSINK_VERSION', '0.0.0')
export SOURCE_DATE_EPOCH := env('SOURCE_DATE_EPOCH', '')
export SOFTSINK_HOMEPAGE := env('SOFTSINK_HOMEPAGE', 'https://github.com/ryanpetris/softsink')
export HOST_ARCHIVE := env('HOST_ARCHIVE', '')
DEB_DISTRIBUTION := env('DEB_DISTRIBUTION', 'trixie')
resolute_base := 'docker.io/library/ubuntu:26.04'
trixie_base := 'docker.io/library/debian:13'
arch_base := 'docker.io/library/archlinux:base'
deb_base := if DEB_DISTRIBUTION == 'trixie' { trixie_base } else if DEB_DISTRIBUTION == 'resolute' { resolute_base } else { '' }
portable := 'build/release/softsink-' + SOFTSINK_VERSION + '-linux-amd64.tar.gz'
arch := 'build/release/softsink-' + SOFTSINK_VERSION + '-1-x86_64.pkg.tar.zst'
deb := 'build/release/softsink_' + SOFTSINK_VERSION + '-1~' + DEB_DISTRIBUTION + '_amd64.deb'
archive := if HOST_ARCHIVE == '' { portable } else { HOST_ARCHIVE }

default:
    @{{ quote(just_executable()) }} --list

package-host-tar: (_host-tar 'resolute' resolute_base portable)

package-host-arch:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "$HOST_ARCHIVE" ]; then {{ quote(just_executable()) }} package-host-tar; fi
    "$DOCKER" build -t softsink-arch -f docker/arch.Dockerfile docker
    {{ quote(just_executable()) }} _build {{ quote(arch) }} -v {{ quote(absolute_path(archive) + ':/input.tar.gz:ro') }} softsink-arch bash packaging/build.sh arch

package-host-deb:
    #!/usr/bin/env bash
    set -euo pipefail
    base={{ quote(deb_base) }}
    test -n "$base" || { echo 'DEB_DISTRIBUTION must be trixie or resolute' >&2; exit 1; }
    archive={{ quote(archive) }}
    if [ {{ quote(DEB_DISTRIBUTION) }} = trixie ]; then
      test -z "$HOST_ARCHIVE" || { echo 'Trixie builds its own executable. Omit HOST_ARCHIVE.' >&2; exit 1; }
      mkdir -p build
      work=$(mktemp -d "$PWD/build/trixie.XXXXXX")
      trap 'rm -rf "$work"' EXIT
      archive="$work/softsink-$SOFTSINK_VERSION-linux-amd64.tar.gz"
      {{ quote(just_executable()) }} _host-tar trixie "$base" "$archive"
    elif [ -z "$HOST_ARCHIVE" ]; then
      {{ quote(just_executable()) }} package-host-tar
    fi
    image=softsink-deb-{{ DEB_DISTRIBUTION }}
    "$DOCKER" build -t "$image" --build-arg "BASE=$base" -f docker/deb.Dockerfile docker
    {{ quote(just_executable()) }} _build {{ quote(deb) }} -v "$(realpath "$archive"):/input.tar.gz:ro" "$image" bash packaging/build.sh deb {{ DEB_DISTRIBUTION }}

check-package-host-tar: package-host-tar
    {{ quote(just_executable()) }} _check {{ resolute_base }} {{ quote(portable) }} tar tar.gz

check-package-arch: package-host-arch
    {{ quote(just_executable()) }} _check {{ arch_base }} {{ quote(arch) }} arch pkg.tar.zst

check-package-deb: package-host-deb
    {{ quote(just_executable()) }} _check {{ deb_base }} {{ quote(deb) }} {{ DEB_DISTRIBUTION }} deb

[private]
_host-tar distribution base output:
    "$DOCKER" build -t softsink-host-{{ distribution }} --build-arg BASE={{ base }} -f docker/host.Dockerfile docker
    {{ quote(just_executable()) }} _build {{ quote(output) }} softsink-host-{{ distribution }} bash packaging/build.sh tar

[positional-arguments]
[private]
_build output +command:
    #!/usr/bin/env bash
    set -euo pipefail
    test "$(uname -sm)" = 'Linux x86_64' || { echo 'Packages require Linux x86-64' >&2; exit 1; }
    python3 tools/version.py >/dev/null
    output=$1
    shift
    mkdir -p "$(dirname "$output")"
    temporary=$(mktemp "$output.XXXXXX")
    trap 'rm -f "$temporary"' EXIT
    "$DOCKER" run --rm -v "$PWD:/src:ro" -w /src \
      -e SOFTSINK_VERSION -e SOURCE_DATE_EPOCH -e SOFTSINK_HOMEPAGE "$@" > "$temporary"
    chmod 644 "$temporary"
    mv -f "$temporary" "$output"

[private]
_check base package kind extension:
    "$DOCKER" run --rm -v "$PWD:/src:ro" -w /src -e SOFTSINK_VERSION \
      -v {{ quote(absolute_path(package) + ':/input.' + extension + ':ro') }} \
      {{ base }} bash packaging/check.sh {{ kind }}
