#!/usr/bin/env python3
"""Create the exact corresponding-source kit for the bundled occt-wasm WASM."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERSION = "4.3.2"
OCCT_WASM_COMMIT = "e5fe93a3b7485d70c905377c3abc9d534aa4c696"
OCCT_COMMIT = "055a9a8a2b3fbb33da2ec9a5445d2b97ffbcd765"
SOURCES = {
    "occt-wasm": (
        f"https://codeload.github.com/andymai/occt-wasm/tar.gz/{OCCT_WASM_COMMIT}",
        "8a51ac789ff2ff51e74136deb630f948d2507cd9755065d473336b5f65175a35",
    ),
    "OCCT": (
        f"https://codeload.github.com/andymai/OCCT/tar.gz/{OCCT_COMMIT}",
        "498f7cdbf6b6d8a5729fc09d3983d418d92532931b21b52c2f2ba285382d8324",
    ),
    "rapidjson": (
        "https://codeload.github.com/Tencent/rapidjson/tar.gz/refs/tags/v1.1.0",
        "bf7ced29704a1e696fbccf2a2b4ea068e7774fa37f6d7dd4039d0787f8bed98e",
    ),
}


def download(url: str, destination: Path, expected: str) -> None:
    # Use the host's TLS store through curl. This avoids Python installations on
    # macOS that lack a configured CA bundle, while checksum verification below
    # still makes the pinned archive identity authoritative.
    subprocess.run(
        ["curl", "--fail", "--location", "--retry", "4", "--retry-delay", "1", "--output", str(destination), url],
        check=True,
    )
    digest = hashlib.sha256(destination.read_bytes())
    actual = digest.hexdigest()
    if actual != expected:
        destination.unlink(missing_ok=True)
        raise RuntimeError(f"checksum mismatch for {url}: expected {expected}, got {actual}")


def extract_snapshot(archive: Path, destination: Path) -> None:
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        prefixes = {member.name.split("/", 1)[0] for member in members if member.name}
        if len(prefixes) != 1:
            raise RuntimeError(f"unexpected archive layout: {archive}")
        prefix = next(iter(prefixes)) + "/"
        for member in members:
            if not member.name.startswith(prefix) or member.issym() or member.islnk():
                if member.issym() or member.islnk():
                    raise RuntimeError(f"archive contains a link: {member.name}")
                continue
            relative = Path(member.name[len(prefix) :])
            if not relative.parts:
                continue
            target = destination / relative
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                extracted = source.extractfile(member)
                if extracted is None:
                    raise RuntimeError(f"cannot extract {member.name}")
                target.write_bytes(extracted.read())
                target.chmod(member.mode & 0o777)


def add_tree(archive: tarfile.TarFile, root: Path, arcroot: str) -> None:
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        info = archive.gettarinfo(str(path), f"{arcroot}/{relative.as_posix()}")
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = 0
        if path.is_file():
            with path.open("rb") as source:
                archive.addfile(info, source)
        else:
            archive.addfile(info)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target" / f"gogglelab-cad-source-{VERSION}.tar.gz")
    parser.add_argument("--cache", type=Path, help="directory containing pre-downloaded occt-wasm.tar.gz and OCCT.tar.gz")
    args = parser.parse_args()
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)

    installed = json.loads((ROOT / "node_modules" / "occt-wasm" / "package.json").read_text())
    if installed.get("version") != VERSION:
        raise RuntimeError(f"expected installed occt-wasm {VERSION}, found {installed.get('version')}")

    with tempfile.TemporaryDirectory(prefix="gogglelab-cad-source-") as temp_name:
        temp = Path(temp_name)
        kit = temp / "kit"
        wrapper = kit / "occt-wasm"
        occt = wrapper / "occt"
        rapidjson = wrapper / "3rdparty" / "rapidjson"
        wrapper.mkdir(parents=True)
        for name, (url, checksum) in SOURCES.items():
            filename = f"{name}.tar.gz"
            source = args.cache / filename if args.cache else temp / filename
            if not source.is_file():
                download(url, source, checksum)
            actual = hashlib.sha256(source.read_bytes()).hexdigest()
            if actual != checksum:
                raise RuntimeError(f"checksum mismatch for {source}: expected {checksum}, got {actual}")
            destinations = {"occt-wasm": wrapper, "OCCT": occt, "rapidjson": rapidjson}
            extract_snapshot(source, destinations[name])

        # Apply the exact, documented upstream build patch so rebuilding does not
        # depend on an unverified network fetch from fetch-rapidjson.sh.
        rapidjson_document = rapidjson / "include" / "rapidjson" / "document.h"
        rapidjson_text = rapidjson_document.read_text(encoding="utf-8")
        old = "GenericStringRef& operator=(const GenericStringRef& rhs) { s = rhs.s; length = rhs.length; }"
        new = "GenericStringRef& operator=(const GenericStringRef& rhs) { this->~GenericStringRef(); new (this) GenericStringRef(rhs); return *this; }"
        if rapidjson_text.count(old) != 1:
            raise RuntimeError("expected RapidJSON 1.1.0 assignment operator was not found exactly once")
        rapidjson_document.write_text(rapidjson_text.replace(old, new), encoding="utf-8", newline="\n")

        gitmodules = (wrapper / ".gitmodules").read_text(encoding="utf-8")
        if "https://github.com/andymai/OCCT.git" not in gitmodules:
            raise RuntimeError("unexpected OCCT submodule origin")
        build = f"""# Rebuilding and replacing the CAD WebAssembly component

This kit is the complete source corresponding to `occt-wasm` {VERSION} shipped by GoggleLab.

- npm package gitHead / occt-wasm commit: `{OCCT_WASM_COMMIT}`
- `occt` submodule commit: `{OCCT_COMMIT}`
- The upstream source archive is expanded at `occt-wasm/` and the exact submodule is expanded at `occt-wasm/occt/`.
- GoggleLab applies no source patch to the occt-wasm or OCCT trees. The shipped WASM is copied from the pinned npm package.
- `occt-wasm/3rdparty/rapidjson/` contains RapidJSON 1.1.0 with the exact compatibility patch
  performed by upstream `scripts/fetch-rapidjson.sh`; its pristine archive hash is in the manifest.

Build without any source download by using the included RapidJSON tree, Rust 1.95+, and emsdk 5.0.3:

```sh
cd occt-wasm
npm install
cd ts && npm install && cd ..
cargo xtask build --release
```

The upstream Docker build calls `scripts/fetch-rapidjson.sh`, which downloads RapidJSON again.
For an offline/auditable build, use the commands above with the included, already patched tree.

To use a rebuilt component, replace the separately distributed `occt-wasm.wasm` resource in
the application bundle with the compatible rebuilt file, retaining its resource name. GoggleLab
loads this WASM as an external runtime resource rather than incorporating it into the executable.
"""
        (kit / "BUILDING-AND-REPLACING.md").write_text(build, encoding="utf-8", newline="\n")
        manifest = {
            "occt-wasm": {"version": VERSION, "commit": OCCT_WASM_COMMIT, "url": SOURCES["occt-wasm"][0], "sha256": SOURCES["occt-wasm"][1]},
            "OCCT": {"commit": OCCT_COMMIT, "url": SOURCES["OCCT"][0], "sha256": SOURCES["OCCT"][1]},
            "RapidJSON": {"version": "1.1.0", "url": SOURCES["rapidjson"][0], "sha256": SOURCES["rapidjson"][1]},
            "patches": ["occt-wasm/scripts/fetch-rapidjson.sh compatibility edit, applied to 3rdparty/rapidjson/include/rapidjson/document.h"],
        }
        (kit / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")

        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
            add_tree(archive, kit, f"gogglelab-cad-source-{VERSION}")
        raw.seek(0)
        with output.open("wb") as destination:
            import gzip
            with gzip.GzipFile(filename="", mode="wb", fileobj=destination, mtime=0) as compressed:
                shutil.copyfileobj(raw, compressed)
    print(f"wrote {output} ({hashlib.sha256(output.read_bytes()).hexdigest()})")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"prepare-cad-source: {error}", file=sys.stderr)
        raise SystemExit(1)
