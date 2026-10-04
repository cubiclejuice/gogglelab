#!/usr/bin/env python3
"""Build deterministic third-party notices from the installed JS and Rust trees."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT
TRACKED_LICENSES = ROOT / "resources" / "licenses"
LICENSE_NAMES = ("license", "copying", "notice", "thirdpartynotice")


@dataclass(frozen=True)
class Component:
    ecosystem: str
    name: str
    version: str
    license_expression: str
    source: str
    license_files: tuple[Path, ...]


def is_license_file(path: Path) -> bool:
    return path.is_file() and any(path.name.lower().startswith(name) for name in LICENSE_NAMES)


def package_license_files(directory: Path) -> tuple[Path, ...]:
    return tuple(sorted((p for p in directory.iterdir() if is_license_file(p)), key=lambda p: p.name.lower()))


def npm_components() -> list[Component]:
    modules = PROJECT / "node_modules"
    if not modules.is_dir():
        raise RuntimeError("node_modules is missing; run bun install --frozen-lockfile first")
    manifests: list[tuple[Path, dict]] = []
    for manifest in modules.rglob("package.json"):
        relative = manifest.relative_to(modules)
        tail = relative.parts[relative.parts.index("node_modules") + 1 :] if "node_modules" in relative.parts else relative.parts
        if not (len(tail) == 2 or (len(tail) == 3 and tail[0].startswith("@"))):
            continue
        data = json.loads(manifest.read_text(encoding="utf-8"))
        manifests.append((manifest, data))

    package_roots: dict[tuple[str, str], list[Path]] = {}
    for manifest, data in manifests:
        name, version = data.get("name"), data.get("version")
        if isinstance(name, str) and isinstance(version, str):
            package_roots.setdefault((name, version), []).append(manifest.parent)

    found: list[Component] = []
    emitted: set[tuple[str, str]] = set()
    for manifest, data in manifests:
        name = data.get("name")
        version = data.get("version")
        license_expression = data.get("license")
        if not all(isinstance(value, str) and value for value in (name, version, license_expression)):
            raise RuntimeError(f"incomplete npm license metadata: {manifest}")
        identity = (name, version)
        if identity in emitted:
            continue
        emitted.add(identity)
        repository = data.get("repository", "")
        if isinstance(repository, dict):
            repository = repository.get("url", "")
        files = package_license_files(manifest.parent)
        # Platform-specific binary packages omit texts that are shipped in their
        # architecture-independent package from the same project and version.
        binary_license_packages = {
            "@esbuild/": "esbuild",
            "@oxlint/binding-": "oxlint",
            "@oxfmt/binding-": "oxfmt",
            "@rollup/rollup-": "rollup",
            "@tauri-apps/cli-": "@tauri-apps/cli",
        }
        for prefix, canonical_name in binary_license_packages.items():
            if name.startswith(prefix) and not files:
                candidates = package_roots.get((canonical_name, version), [])
                if not candidates:
                    raise RuntimeError(
                        f"npm binary package {name}@{version} has no exact installed canonical package {canonical_name}@{version}"
                    )
                files = package_license_files(sorted(candidates, key=str)[0])
                break
        if name == "occt-wasm":
            files = (
                TRACKED_LICENSES / "occt-wasm-MIT.txt",
                TRACKED_LICENSES / "occt-wasm-APACHE-2.0.txt",
                TRACKED_LICENSES / "LGPL-2.1-only.txt",
                TRACKED_LICENSES / "OCCT-LGPL-exception-1.0.txt",
            )
            license_expression = "MIT OR Apache-2.0 (wrapper); LGPL-2.1-only with OCCT exception (WASM)"
        if not files:
            raise RuntimeError(f"npm package has no installed license text: {name}@{version}")
        found.append(Component("npm", name, version, license_expression, str(repository), files))
    return found


def cargo_components() -> list[Component]:
    proc = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=PROJECT,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    )
    metadata = json.loads(proc.stdout)
    found: list[Component] = []
    for package in metadata["packages"]:
        if not package.get("source"):
            continue
        manifest = Path(package["manifest_path"])
        license_expression = package.get("license") or ""
        explicit = package.get("license_file")
        files = (manifest.parent / explicit,) if explicit else package_license_files(manifest.parent)
        if not license_expression:
            raise RuntimeError(f"Rust crate has no license expression: {package['name']} {package['version']}")
        if not files:
            # Some crates inherit a workspace license but omit the repository-level
            # files from their crates.io tarball. Supply the complete standard texts
            # for every identifier in the declared expression.
            standards = {
                "Apache-2.0": TRACKED_LICENSES / "Apache-2.0.txt",
                "BSD-3-Clause": TRACKED_LICENSES / "BSD-3-Clause.txt",
                "LGPL-2.1": TRACKED_LICENSES / "LGPL-2.1-only.txt",
                "MIT": TRACKED_LICENSES / "MIT.txt",
                "MPL-2.0": TRACKED_LICENSES / "MPL-2.0.txt",
                "Zlib": TRACKED_LICENSES / "Zlib.txt",
            }
            files = tuple(path for identifier, path in standards.items() if identifier in license_expression)
        if not files or any(not path.is_file() for path in files):
            raise RuntimeError(f"Rust crate has no available license text: {package['name']} {package['version']}")
        found.append(
            Component(
                "cargo",
                package["name"],
                package["version"],
                license_expression,
                package.get("repository") or str(package["source"]),
                files,
            )
        )
    return found


def safe_reset(destination: Path) -> None:
    resolved = destination.resolve()
    if resolved in (ROOT.resolve(), Path(resolved.anchor)) or len(resolved.parts) < 4:
        raise RuntimeError(f"refusing unsafe output directory: {resolved}")
    if destination.exists():
        shutil.rmtree(destination)
    (destination / "texts").mkdir(parents=True)


def main() -> int:
    global PROJECT
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", type=Path, default=ROOT, help="edition root containing node_modules and Cargo.toml")
    parser.add_argument("--output", type=Path, help="defaults to <project>/resources/licenses/generated")
    args = parser.parse_args()
    PROJECT = args.project.resolve()
    if not (PROJECT / "package.json").is_file() or not (PROJECT / "Cargo.toml").is_file():
        raise RuntimeError(f"project is missing package.json or Cargo.toml: {PROJECT}")
    destination = (args.output or PROJECT / "resources" / "licenses" / "generated").resolve()
    components = sorted(npm_components() + cargo_components(), key=lambda c: (c.ecosystem, c.name, c.version))
    safe_reset(destination)

    text_names: dict[str, str] = {}
    records = []
    for component in components:
        refs = []
        for path in component.license_files:
            content = path.read_bytes().replace(b"\r\n", b"\n")
            digest = hashlib.sha256(content).hexdigest()
            filename = f"{digest}.txt"
            if digest not in text_names:
                (destination / "texts" / filename).write_bytes(content)
                text_names[digest] = filename
            refs.append(f"texts/{filename}")
        records.append({
            "ecosystem": component.ecosystem,
            "name": component.name,
            "version": component.version,
            "license": component.license_expression,
            "source": component.source,
            "license_texts": sorted(set(refs)),
        })

    lines = [
        "# GoggleLab third-party notices",
        "",
        "This distribution includes the components below. Each component retains its own license.",
        "The referenced files under `texts/` contain the complete license and notice text found in",
        "the exact installed package used to build this release.",
        "",
        "`occt-wasm` contains a separately loaded Open CASCADE WebAssembly component. Its corresponding",
        "source kit and replacement instructions are distributed alongside the application.",
        "",
    ]
    for record in records:
        lines.extend([
            f"## {record['name']} {record['version']} ({record['ecosystem']})",
            "",
            f"License: {record['license']}",
            f"Source: {record['source'] or 'recorded in the package lockfile'}",
            "License text: " + ", ".join(f"`{ref}`" for ref in record["license_texts"]),
            "",
        ])
    (destination / "THIRD-PARTY-NOTICES.md").write_text("\n".join(lines), encoding="utf-8", newline="\n")
    (destination / "manifest.json").write_text(json.dumps(records, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {len(records)} component records and {len(text_names)} unique license texts to {destination}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"prepare-licenses: {error}", file=sys.stderr)
        raise SystemExit(1)
