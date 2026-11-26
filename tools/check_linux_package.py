"""Check package contents and runtime libraries without opening a window."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

from check_release_archive import DESKTOP, DOCUMENTS, PORTABLE, check_elf


def run(*command):
    return subprocess.check_output(command, text=True).strip()


def check_files(listed, expected):
    if listed != expected:
        raise ValueError(f"Missing package files: {sorted(expected - listed)}; "
                         f"unexpected package files: {sorted(listed - expected)}")
    for path in listed:
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"Expected a regular package file: {path}")


def check(kind, version):
    if kind == "tar":
        doc = Path.cwd()
        binary = doc / "softsink"
        desktop = doc / f"{DESKTOP}.desktop"
        listed = {path for path in doc.rglob("*") if not path.is_dir()}
        expected = {doc / name for name in PORTABLE}
    else:
        doc = Path("/usr/share/doc/softsink")
        binary = Path("/usr/bin/softsink")
        desktop = Path(f"/usr/share/applications/{DESKTOP}.desktop")
        expected = {doc / name for name in DOCUMENTS} | {
            binary, desktop,
            Path(f"/usr/share/icons/hicolor/512x512/apps/{DESKTOP}.png"),
            Path(f"/usr/share/icons/hicolor/scalable/apps/{DESKTOP}.svg"),
        }
        if kind == "arch":
            listing = run("pacman", "-Qlq", "softsink")
            installed = run("pacman", "-Q", "softsink")
            if installed != f"softsink {version}-1":
                raise ValueError(f"Unexpected package version: {installed}")
            expected.add(Path("/usr/share/licenses/softsink/LICENSE"))
        else:
            listing = run("dpkg-query", "-L", "softsink")
            installed = run("dpkg-query", "-W", "-f=${Version}", "softsink")
            if installed != f"{version}-1~{kind}":
                raise ValueError(f"Unexpected package version: {installed}")
            expected.update(doc / name for name in ("copyright", "changelog.Debian.gz"))
        listed = {Path(line) for line in listing.splitlines() if not Path(line).is_dir()}

    inventory = json.loads((doc / "dependencies.json").read_text())
    bundled = {component["name"] for component in inventory["bundled"] if component["notices"]}
    if bundled != {"Inter", "Phosphor Icons", "PipeWire SPA headers", "libjpeg-turbo"} or not inventory["crates"]:
        raise ValueError("Incomplete dependency inventory")
    for crate in inventory["crates"]:
        if not crate["notices"]:
            raise ValueError(f"Missing dependency notice: {crate['name']} {crate['version']}")
    notices = inventory["rust_notices"] + [
        notice for component in inventory["crates"] + inventory["bundled"]
        for notice in component["notices"]
    ]
    check_files(listed, expected | {doc / notice["file"] for notice in notices})
    for notice in notices:
        if hashlib.sha256((doc / notice["file"]).read_bytes()).hexdigest() != notice["sha256"]:
            raise ValueError(f"Incorrect notice contents: {notice['file']}")

    if (doc / "VERSION").read_text() != version + "\n":
        raise ValueError("Incorrect contents in VERSION")
    entry = desktop.read_text().splitlines()
    if not {"Exec=softsink", f"Icon={DESKTOP}", f"StartupWMClass={DESKTOP}"} <= set(entry):
        raise ValueError("Incorrect desktop entry")
    with binary.open("rb") as stream:
        check_elf(stream.read(20))
    if not os.access(binary, os.X_OK):
        raise ValueError("The packaged binary must be executable")
    linked = run("ldd", str(binary))
    if "not found" in linked or "libturbojpeg" in linked:
        raise ValueError(f"Unexpected runtime linkage:\n{linked}")
    for library in ("libvulkan.so.1", "libwayland-client.so.0", "libxkbcommon.so.0",
                    "libpipewire-0.3.so.0", "libasound.so.2"):
        ctypes.CDLL(library)
    if run(str(binary), "--version") != f"softsink {version}":
        raise ValueError("The executable version does not match the release")
    print(f"Checked {kind}: softsink {version}, {len(listed)} files, and runtime libraries")


if __name__ == "__main__":
    check(*sys.argv[1:])
