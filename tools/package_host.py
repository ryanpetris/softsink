"""Package the Linux executable, desktop files, documentation, and notices."""
import argparse
import gzip
import hashlib
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import time

import dependency_notices
from version import version


def package(binary, output):
    release = version()
    name = f"softsink-{release}-linux-amd64"
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH") or time.time())
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output) as temporary:
        root = Path(temporary) / name
        root.mkdir()
        shutil.copyfile(binary, root / "softsink")
        shutil.copyfile("packaging/dev.petris.softsink.desktop", root / "dev.petris.softsink.desktop")
        shutil.copyfile("packaging/icons/app-512.png", root / "dev.petris.softsink.png")
        shutil.copyfile("packaging/icons/app.svg", root / "dev.petris.softsink.svg")
        for source in ("LICENSE", "README.md"):
            shutil.copyfile(source, root / source)
        shutil.copytree("docs", root / "docs")
        (root / "VERSION").write_text(release + "\n", encoding="ascii")
        dependency_notices.record(root)
        files = sorted(path for path in root.rglob("*") if path.is_file())
        (root / "SHA256SUMS").write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  ./{path.relative_to(root).as_posix()}\n"
            for path in files), encoding="utf-8")

        def archive_metadata(member):
            member.uid = member.gid = 0
            member.uname = member.gname = ""
            member.mtime = epoch
            member.mode = 0o755 if member.isdir() or member.name == f"{name}/softsink" else 0o644
            return member

        archive = Path(temporary) / f"{name}.tar.gz"
        with archive.open("wb") as stream, gzip.GzipFile(
                filename="", mode="wb", fileobj=stream, mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as tar:
                tar.add(root, arcname=name, filter=archive_metadata)
        archive.replace(output / archive.name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    package(args.binary, args.output)
