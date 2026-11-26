"""Check the portable archive before extracting it for native packaging."""
import hashlib
from pathlib import PurePosixPath
import sys
import tarfile

DOCUMENTS = {"README.md", "docs/building.md", "docs/installation.md",
             "docs/usage.md", "DEPENDENCIES.md", "dependencies.json", "VERSION"}
DESKTOP = "dev.petris.softsink"
PORTABLE = DOCUMENTS | {"softsink", "LICENSE", "SHA256SUMS",
                        f"{DESKTOP}.desktop", f"{DESKTOP}.png", f"{DESKTOP}.svg"}


def check_elf(header):
    if header[:6] != b"\x7fELF\x02\x01" or header[18:20] != b"\x3e\x00":
        raise ValueError("Distribution packages require a Linux x86-64 executable")


def check(path, version):
    prefix = f"softsink-{version}-linux-amd64"
    with tarfile.open(path, "r:gz") as archive:
        files = {}
        seen = set()
        for member in archive:
            name = PurePosixPath(member.name)
            if not name.parts or name.parts[0] != prefix or ".." in name.parts or name in seen:
                raise ValueError(f"Unexpected archive path: {member.name}")
            seen.add(name)
            relative = str(name.relative_to(prefix))
            mode = 0o755 if member.isdir() or relative == "softsink" else 0o644
            if not (member.isdir() or member.isfile()) or member.mode != mode:
                raise ValueError(f"Unexpected archive type or mode: {member.name}")
            if member.isfile():
                files[relative] = member
        if not PORTABLE <= files.keys():
            raise ValueError(f"Missing archive files: {sorted(PORTABLE - files.keys())}")
        if any(not name.startswith("notices/") for name in files.keys() - PORTABLE):
            raise ValueError("Unexpected files in the archive")
        if archive.extractfile(files["VERSION"]).read() != version.encode("ascii") + b"\n":
            raise ValueError("Archive version does not match SOFTSINK_VERSION")
        check_elf(archive.extractfile(files["softsink"]).read(20))
        checksums = "".join(
            f"{hashlib.sha256(archive.extractfile(files[name]).read()).hexdigest()}  ./{name}\n"
            for name in sorted(files.keys() - {"SHA256SUMS"})
        )
        if archive.extractfile(files["SHA256SUMS"]).read().decode("utf-8") != checksums:
            raise ValueError("Archive checksums do not match its payload")
        return int(files["VERSION"].mtime)


if __name__ == "__main__":
    archive, version = sys.argv[1:]
    print(check(archive, version))
