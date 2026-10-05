#!/usr/bin/env python3
"""Package one native runner's release downloads and provenance manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys


APPLICATION_IDENTIFIER = "com.sonicparke.stl-handoff"
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")


def asset_base(edition: str, version: str, platform: str, arch: str) -> str:
    if edition == "official":
        return f"gogglelab-{version}-{platform}-{arch}"
    return f"gogglelab-{edition}-{version}-{platform}-{arch}"


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_payload(
    *,
    edition: str,
    version: str,
    target: str,
    platform: str,
    arch: str,
    assets: dict[str, str],
    private_build_commit: str | None,
    public_core_commit: str | None,
) -> dict[str, object]:
    if edition != "official":
        return {"edition": edition, "version": version, "target": target, "assets": assets}
    if not private_build_commit or not COMMIT_PATTERN.fullmatch(private_build_commit):
        raise ValueError("official packages require an exact lowercase private build commit")
    if not public_core_commit or not COMMIT_PATTERN.fullmatch(public_core_commit):
        raise ValueError("official packages require an exact lowercase public core commit")
    return {
        "schema": 1,
        "product": "gogglelab",
        "version": version,
        "application_identifier": APPLICATION_IDENTIFIER,
        "platform": platform,
        "arch": arch,
        "target": target,
        "private_build_commit": private_build_commit,
        "public_core_commit": public_core_commit,
        "assets": assets,
    }


def exactly_one(directory: Path, pattern: str) -> Path:
    matches = list(directory.glob(pattern))
    if len(matches) != 1:
        raise RuntimeError(f"expected exactly one {pattern} in {directory}, found {matches}")
    return matches[0]


def package_downloads(
    *,
    root: Path,
    out: Path,
    base: str,
    target: str,
    platform: str,
) -> list[Path]:
    bundle = root / "target" / target / "release" / "bundle"
    packaged: list[Path] = []
    if platform == "macos":
        app = exactly_one(bundle / "macos", "*.app")
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        destination = out / f"{base}.zip"
        subprocess.run(
            ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(destination)],
            check=True,
        )
        packaged.append(destination)
    elif platform == "windows":
        for folder, pattern, suffix in (
            ("nsis", "*.exe", "-setup.exe"),
            ("msi", "*.msi", ".msi"),
        ):
            destination = out / f"{base}{suffix}"
            shutil.copy2(exactly_one(bundle / folder, pattern), destination)
            packaged.append(destination)
    elif platform == "linux":
        for folder, pattern, suffix in (
            ("deb", "*.deb", ".deb"),
            ("appimage", "*.AppImage", ".AppImage"),
        ):
            destination = out / f"{base}{suffix}"
            shutil.copy2(exactly_one(bundle / folder, pattern), destination)
            packaged.append(destination)
    else:
        raise ValueError(f"unsupported platform: {platform}")
    return packaged


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--edition", required=True, choices=("community", "pro", "official"))
    result.add_argument("--target", required=True)
    result.add_argument("--platform", required=True, choices=("macos", "windows", "linux"))
    result.add_argument("--arch", required=True, choices=("arm64", "x64"))
    result.add_argument("--private-build-commit")
    result.add_argument("--public-core-commit")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    root = Path.cwd()
    version = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    out = root / "release-assets"
    out.mkdir(exist_ok=True)
    base = asset_base(args.edition, version, args.platform, args.arch)
    packaged = package_downloads(
        root=root,
        out=out,
        base=base,
        target=args.target,
        platform=args.platform,
    )
    assets = {path.name: file_sha256(path) for path in packaged}
    manifest = manifest_payload(
        edition=args.edition,
        version=version,
        target=args.target,
        platform=args.platform,
        arch=args.arch,
        assets=assets,
        private_build_commit=args.private_build_commit,
        public_core_commit=args.public_core_commit,
    )
    manifest_path = out / f"{base}-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"package-release: {error}", file=sys.stderr)
        raise SystemExit(1)
