"""Record the selected Cargo graph and preserve its redistribution notices."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

TARGET = "x86_64-unknown-linux-gnu"


def record(output):
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", TARGET,
    ]))
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending, selected = [metadata["resolve"]["root"]], set()
    while pending:
        package = pending.pop()
        if package in selected:
            continue
        selected.add(package)
        pending.extend(dep["pkg"] for dep in nodes[package]["deps"]
                       if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]))

    def copy(source, name):
        destination = output / "notices" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
        return {"file": str(destination.relative_to(output)),
                "sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}

    notice_sources = Path("packaging/notices")
    crates = []
    roots = {}
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in selected or package["id"] == metadata["resolve"]["root"]:
            continue
        root = Path(package["manifest_path"]).parent
        roots[package["name"]] = root
        prefix = f"crates/{package['name']}-{package['version']}"
        paths = {path for path in root.iterdir() if path.is_file() and path.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"))}
        if package["license_file"]:
            paths.add(root / package["license_file"])
        texts = [copy(path, f"{prefix}/{path.relative_to(root)}") for path in sorted(paths)]
        if not texts:
            name = package["name"]
            project = "egui" if name in ("ecolor", "egui-wgpu", "egui-winit", "emath", "epaint") else name
            texts = [copy(path, f"{prefix}/{path.name}")
                     for path in sorted((notice_sources / "crates" / project).glob("*"))]
        vcs = root / ".cargo_vcs_info.json"
        crates.append({
            **{key: package[key] for key in ("name", "version", "license", "authors", "repository", "source")},
            "features": nodes[package["id"]]["features"],
            "notices": texts,
            "vcs": json.loads(vcs.read_text()) if vcs.is_file() else None,
        })

    jpeg = roots["turbojpeg-sys"] / "libjpeg-turbo"
    jpeg_version = re.search(r"^set\(VERSION ([\d.]+)\)", (jpeg / "CMakeLists.txt").read_text(),
                             re.MULTILINE)[1]
    rust = subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip()
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    rust_docs = sysroot / "share/doc/rust"
    rust_notices = [copy(path, "rust/" + str(path.relative_to(rust_docs)))
                    for path in sorted(rust_docs.glob("COPYRIGHT*.html"))
                    + sorted((rust_docs / "licenses").glob("*")) if path.is_file()]
    bundled = [
        {"name": "Inter", "license": "OFL-1.1",
         "notices": [copy(Path("assets/Inter-LICENSE.txt"), "inter/LICENSE.txt")]},
        {"name": "Phosphor Icons", "license": "MIT", "version": "2.1",
         "notices": [copy(notice_sources / "Phosphor-LICENSE.txt", "phosphor/LICENSE.txt")]},
        {"name": "PipeWire SPA headers", "license": "MIT",
         "notices": [copy(notice_sources / "PipeWire-COPYING.txt", "pipewire-spa/COPYING")]},
        {"name": "libjpeg-turbo", "version": jpeg_version, "license": "IJG and BSD-3-Clause and Zlib",
         "notices": [copy(jpeg / name, f"libjpeg-turbo/{name}") for name in ("LICENSE.md", "README.ijg")]},
    ]
    inventory = {"target": TARGET, "rustc": rust, "crates": crates,
                 "bundled": bundled, "rust_notices": rust_notices}
    (output / "dependencies.json").write_text(json.dumps(inventory, indent=2) + "\n")
    (output / "DEPENDENCIES.md").write_text(
        "# Dependency inventory\n\n"
        "`dependencies.json` lists the selected Rust dependencies, including build dependencies, "
        "and the fonts and native code bundled in the executable. "
        "`notices/` contains the available license and notice files.\n\n"
        "This software is based in part on the work of the Independent JPEG Group.\n"
    )
