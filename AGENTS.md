# Instructions for agents working on Softsink

## Third-party notices

Keep third-party notices current for every Rust dependency and bundled component built into the binary. `tools/package_host.py` calls `tools/dependency_notices.py` to generate `notices/`, `dependencies.json`, and `DEPENDENCIES.md`, and `tools/check_linux_package.py` validates them during package checks.

### When to update

Review third-party notices whenever you change any of the following:

- Dependencies in `Cargo.toml` or `Cargo.lock`.
- `assets/Inter-Regular.ttf`.
- `egui-phosphor` (or the Phosphor Icons font it embeds).
- `pipewire`, `libspa-sys`, or the base images set by `resolute_base` and `trixie_base` in `justfile` (which override the default in `docker/host.Dockerfile` and supply `libpipewire-0.3-dev`).
- `turbojpeg` or `turbojpeg-sys` (which bundles `libjpeg-turbo`).
- `RUSTUP_TOOLCHAIN` in `docker/host.Dockerfile`.

`tools/dependency_notices.py` resolves the `x86_64-unknown-linux-gnu` target graph and skips dev-only dependencies.

### Finding missing notices

`tools/dependency_notices.py` runs `cargo metadata --locked --format-version 1 --filter-platform x86_64-unknown-linux-gnu` and copies regular files in each crate root whose uppercase name starts with `LICENSE`, `LICENCE`, `COPYING`, `NOTICE`, or `UNLICENSE`, plus `license_file` if set. Crates shipping none of those fall back to `packaging/notices/crates/<project>/`.

Find missing crate notices either by running `just check-package-host-tar` (fails with `Missing dependency notice: <name> <version>`) or with a fast host check after `cargo fetch --locked --target x86_64-unknown-linux-gnu`:

```sh
python3 -c '
import json, pathlib, sys, tempfile
sys.path.insert(0, "tools")
import dependency_notices

with tempfile.TemporaryDirectory() as tmp:
    out = pathlib.Path(tmp)
    dependency_notices.record(out)
    data = json.loads((out / "dependencies.json").read_text())
    for crate in data["crates"]:
        if not crate["notices"]:
            print(crate["name"], crate["version"], crate["license"])
'
```

`tools/check_linux_package.py` only fails when a crate has no notices at all; it does not warn when a fallback crate updates, leaves the graph, or starts shipping its own license file. On every `Cargo.lock` change, audit every directory under `packaging/notices/crates/` against the selected crates and their Cargo cache roots.

### Adding, updating, and removing crate fallbacks

For each selected crate that ships no license file:

1. Locate the upstream repository and exact revision from `dependencies.json` (`repository` and `vcs`) or `.cargo_vcs_info.json` in Cargo's cache.
2. Copy all matching license and third-party notice files from that release tag or commit (matching the crate's declared `license` field; include dual-license files and extra notices such as `accesskit`'s `LICENSE.chromium`).
3. Place files directly in `packaging/notices/crates/<project>/` with no subdirectories (`tools/dependency_notices.py` copies `glob("*")` with `shutil.copyfile`).
4. Map shared-repo crates in `tools/dependency_notices.py` if needed (default `<project>` is the crate name; `ecolor`, `egui-wgpu`, `egui-winit`, `emath`, and `epaint` currently share `packaging/notices/crates/egui/`).
5. If `Cargo.lock` resolves multiple versions of a crate, verify the fallback applies to every resolved version or split the mapping in `tools/dependency_notices.py` if terms diverge.

When a crate leaves the graph or starts shipping its own license file, delete its `packaging/notices/crates/<project>/` directory only when no other selected crate or version uses it, and remove its name from any shared `project` mapping in `tools/dependency_notices.py`.

### Bundled components and Rust notices

`tools/dependency_notices.py` records four bundled non-Rust components and `tools/check_linux_package.py` requires that exact set. Update both scripts if you add, rename, or remove a component.

| Component | Source and files to check on update |
| --- | --- |
| `Inter` | Update `assets/Inter-LICENSE.txt` from the matching Inter release and verify `OFL-1.1` in `tools/dependency_notices.py`. |
| `Phosphor Icons` | Check which release `egui-phosphor` embeds, update `version` and `license` in `tools/dependency_notices.py`, and update `packaging/notices/Phosphor-LICENSE.txt`. |
| `PipeWire SPA headers` | Check PipeWire's `COPYING` for the `libpipewire-0.3-dev` version installed in `resolute_base` and `trixie_base`, update `packaging/notices/PipeWire-COPYING.txt` if changed, and verify `MIT` in `tools/dependency_notices.py`. |
| `libjpeg-turbo` | Check that `set(VERSION ...)` in `libjpeg-turbo/CMakeLists.txt`, `LICENSE.md`, and `README.ijg` still exist in `turbojpeg-sys`, verify `IJG and BSD-3-Clause and Zlib` matches `LICENSE.md`, and check the IJG attribution sentence in `DEPENDENCIES.md`. |

When changing `RUSTUP_TOOLCHAIN` in `docker/host.Dockerfile`, verify that `rust_notices` in `dependencies.json` (copied from `share/doc/rust` under `rustc --print sysroot`) remains populated.

### Verification

See [Building Softsink](docs/building.md) for prerequisites, `SOFTSINK_VERSION`, `HOST_ARCHIVE`, and `DOCKER=podman`. Rebuild and validate all package targets:

```sh
just check-package-host-tar
export HOST_ARCHIVE="build/release/softsink-${SOFTSINK_VERSION:-0.0.0}-linux-amd64.tar.gz"
just check-package-arch
just DEB_DISTRIBUTION=resolute check-package-deb
unset HOST_ARCHIVE
just DEB_DISTRIBUTION=trixie check-package-deb
```

## Documentation packaging

`tools/package_host.py` copies `README.md` and `docs/` into release archives, and `tools/check_release_archive.py` checks the exact `DOCUMENTS` set. Keep `DOCUMENTS` in `tools/check_release_archive.py` aligned whenever files are added to or removed from `docs/`.
