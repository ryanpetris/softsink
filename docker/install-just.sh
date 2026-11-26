#!/bin/sh
# Install the pinned just release after verifying its archive.
set -eu
version=1.59.0
target=x86_64-unknown-linux-musl
checksum=409044617b6f06517c1035e55ca38c8c856ff8b650e41bc2aa1ef82d5903a23e
output=$1
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "https://github.com/casey/just/releases/download/$version/just-$version-$target.tar.gz" \
  --output "$temporary/just.tar.gz"
printf '%s  %s\n' "$checksum" "$temporary/just.tar.gz" | sha256sum --check
tar -xzf "$temporary/just.tar.gz" -C "$temporary" just
install -Dm755 "$temporary/just" "$output/just"
