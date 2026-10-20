"""License inventory regressions using metadata fixtures; never invokes Cargo."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "gogglelab_prepare_licenses", Path(__file__).with_name("prepare-licenses.py")
)
LICENSES = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = LICENSES
SPEC.loader.exec_module(LICENSES)


class CargoLicenseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="gogglelab-license-fixture-")
        self.addCleanup(temporary.cleanup)
        self.project = Path(temporary.name)

    def package(self, name, directory, *, source=None, repository=None, with_license=True):
        directory.mkdir(parents=True)
        manifest = directory / "Cargo.toml"
        manifest.write_text("# Synthetic metadata fixture\n", encoding="utf-8")
        if with_license:
            (directory / "LICENSE").write_text(f"Synthetic license for {name}\n", encoding="utf-8")
        return {
            "name": name,
            "version": "1.0.0",
            "manifest_path": str(manifest),
            "license": "MIT",
            "source": source,
            "repository": repository,
        }

    def collect(self, packages, *, root=None, project=None):
        metadata = SimpleNamespace(stdout=json.dumps({"packages": packages}))
        with patch.object(LICENSES, "ROOT", root or self.project), patch.object(
            LICENSES, "PROJECT", project or self.project
        ), patch.object(LICENSES.subprocess, "run", return_value=metadata) as run:
            components = LICENSES.cargo_components()
        run.assert_called_once()
        return components

    def test_vendored_license_is_retained_alongside_registry_dependencies(self):
        vendor = self.package("zip", self.project / "vendor" / "zip", repository="https://example.test/zip")
        registry = self.package("registry", self.project / "registry", source="registry+synthetic")
        own = self.package("application", self.project / "crates" / "application")
        neighbor = self.package("neighbor", self.project / "vendor-neighbor" / "neighbor")
        components = self.collect([vendor, registry, own, neighbor])
        self.assertEqual([component.name for component in components], ["zip", "registry"])
        self.assertEqual(components[0].source, "https://example.test/zip")
        self.assertEqual(components[0].license_files, ((self.project / "vendor" / "zip" / "LICENSE").resolve(),))
        self.assertEqual(components[0].license_files[0].read_text(), "Synthetic license for zip\n")
        with patch.object(LICENSES, "ROOT", self.project), patch.object(LICENSES, "PROJECT", self.project):
            self.assertEqual(LICENSES.approved_vendor_roots(), ((self.project / "vendor").resolve(),))

    def test_missing_repository_does_not_expose_local_vendor_path(self):
        package = self.package("zip", self.project / "vendor" / "zip")
        component, = self.collect([package])
        self.assertEqual(component.source, "vendored path dependency")
        self.assertNotIn(str(self.project), component.source)

    def test_vendored_dependency_without_license_metadata_fails_closed(self):
        package = self.package("zip", self.project / "vendor" / "zip")
        package["license"] = None
        with self.assertRaisesRegex(RuntimeError, "Rust crate has no license expression: zip"):
            self.collect([package])

    def test_composed_core_and_edition_vendors_keep_complete_texts(self):
        core = self.project / "core"
        core_vendor = self.package("core-zip", core / "vendor" / "zip")
        edition_vendor = self.package("edition-tool", self.project / "vendor" / "tool")
        unrelated = self.package("unrelated", self.project / "other-core" / "vendor" / "zip")
        components = self.collect([core_vendor, edition_vendor, unrelated], root=core)
        self.assertEqual([component.name for component in components], ["core-zip", "edition-tool"])
        self.assertEqual(
            [component.license_files[0].read_text() for component in components],
            ["Synthetic license for core-zip\n", "Synthetic license for edition-tool\n"],
        )
        self.assertTrue(all(component.source == "vendored path dependency" for component in components))

    def test_composed_local_proprietary_and_external_paths_stay_omitted(self):
        core = self.project / "core"
        own = self.package("private-app", self.project / "crates" / "app")
        core_own = self.package("core-app", core / "crates" / "app")
        neighbor = self.package("neighbor", core / "vendor-neighbor" / "tool")
        external = self.package("external", self.project / "external" / "dependency")
        registry = self.package("registry", self.project / "registry", source="registry+synthetic")
        components = self.collect([own, core_own, neighbor, external, registry], root=core)
        self.assertEqual([component.name for component in components], ["registry"])
        self.assertEqual(components[0].license_files[0].read_text(), "Synthetic license for registry\n")

    def test_composed_vendor_crate_and_manifest_symlink_escapes_stay_omitted(self):
        core = self.project / "core"
        safe = self.package("safe", core / "vendor" / "safe")
        external = self.package("external", self.project / "external" / "zip")
        crate_link = core / "vendor" / "crate-link"
        manifest_link = core / "vendor" / "manifest-link"
        manifest_link.mkdir()
        try:
            crate_link.symlink_to(Path(external["manifest_path"]).parent, target_is_directory=True)
            (manifest_link / "Cargo.toml").symlink_to(external["manifest_path"])
        except (OSError, NotImplementedError) as error:
            self.skipTest(f"symlink fixtures unavailable: {error}")
        crate_package = dict(external, manifest_path=str(crate_link / "Cargo.toml"))
        manifest_package = dict(external, manifest_path=str(manifest_link / "Cargo.toml"))
        components = self.collect([safe, external, crate_package, manifest_package], root=core)
        self.assertEqual([component.name for component in components], ["safe"])
        self.assertEqual(components[0].license_files[0].read_text(), "Synthetic license for safe\n")

    def test_symlinked_vendor_roots_cannot_authorize_external_trees(self):
        for escaped_root in ["edition", "core"]:
            with self.subTest(escaped_root=escaped_root):
                project = self.project / escaped_root / "edition"
                core = project / "core"
                core.mkdir(parents=True)
                outside = self.project / escaped_root / "outside"
                external = self.package("external", outside / "zip")
                bad_base, safe_base = (project, core) if escaped_root == "edition" else (core, project)
                safe = self.package("safe", safe_base / "vendor" / "safe")
                try:
                    (bad_base / "vendor").symlink_to(outside, target_is_directory=True)
                except (OSError, NotImplementedError) as error:
                    self.skipTest(f"symlink fixtures unavailable: {error}")
                alias = dict(external, manifest_path=str(bad_base / "vendor" / "zip" / "Cargo.toml"))
                components = self.collect([safe, external, alias], root=core, project=project)
                self.assertEqual([component.name for component in components], ["safe"])
                self.assertEqual(components[0].license_files[0].read_text(), "Synthetic license for safe\n")

    def test_reverse_manifest_alias_reads_vendor_license_not_outside_neighbor(self):
        core = self.project / "core"
        vendor = self.package("zip", core / "vendor" / "zip")
        outside = self.project / "outside"
        outside.mkdir()
        (outside / "LICENSE").write_text("Outside private sentinel\n", encoding="utf-8")
        try:
            (outside / "Cargo.toml").symlink_to(vendor["manifest_path"])
        except (OSError, NotImplementedError) as error:
            self.skipTest(f"symlink fixtures unavailable: {error}")
        alias = dict(vendor, manifest_path=str(outside / "Cargo.toml"))
        component, = self.collect([alias], root=core)
        self.assertEqual(component.license_files, ((core / "vendor" / "zip" / "LICENSE").resolve(),))
        self.assertEqual(component.license_files[0].read_text(), "Synthetic license for zip\n")

    def test_vendored_license_escape_fails_closed_instead_of_standard_text_fallback(self):
        for mode in ["explicit", "matched-symlink", "dangling-symlink", "other-approved-root"]:
            with self.subTest(mode=mode):
                core = self.project / mode / "core"
                package = self.package("zip", core / "vendor" / "zip", with_license=False)
                outside = self.project / mode / "outside"
                outside.mkdir()
                private = outside / "PRIVATE-LICENSE"
                private.write_text("Outside private sentinel\n", encoding="utf-8")
                if mode == "other-approved-root":
                    shared = self.project / "vendor" / "shared"
                    shared.mkdir(parents=True)
                    text = shared / "MIT.txt"
                    text.write_text("Other approved root text\n", encoding="utf-8")
                    package["license_file"] = str(text)
                elif mode == "explicit":
                    package["license_file"] = str(private)
                else:
                    target = private if mode == "matched-symlink" else outside / "MISSING"
                    try:
                        (core / "vendor" / "zip" / "LICENSE").symlink_to(target)
                    except (OSError, NotImplementedError) as error:
                        self.skipTest(f"symlink fixtures unavailable: {error}")
                with self.assertRaisesRegex(RuntimeError, "Vendored license text escapes approved vendor root"):
                    self.collect([package], root=core)

    def test_nested_approved_vendor_roots_fail_closed(self):
        core = self.project / "core"
        edition = core / "vendor" / "edition"
        outer = self.package("outer", core / "vendor" / "outer")
        inner = self.package("inner", edition / "vendor" / "inner")
        outer["license_file"] = str(Path(inner["manifest_path"]).parent / "LICENSE")
        with self.assertRaisesRegex(RuntimeError, "Nested approved vendor roots are unsupported"):
            self.collect([outer, inner], root=core, project=edition)

    def test_vendored_shared_license_within_its_approved_root_is_allowed(self):
        core = self.project / "core"
        vendor = core / "vendor"
        package = self.package("zip", vendor / "zip", with_license=False)
        shared = vendor / "SHARED-MIT.txt"
        shared.write_text("Shared complete vendor text\n", encoding="utf-8")
        package["license_file"] = "../SHARED-MIT.txt"
        component, = self.collect([package], root=core)
        self.assertEqual(component.license_files[0].read_text(), "Shared complete vendor text\n")

    def test_registry_explicit_license_and_tracked_standard_fallback_keep_existing_policy(self):
        core = self.project / "core"
        vendor = self.package("zip", core / "vendor" / "zip", with_license=False)
        registry = self.package("registry", self.project / "registry", source="registry+synthetic", with_license=False)
        outside = self.project / "registry-text.txt"
        outside.write_text("Explicit registry text\n", encoding="utf-8")
        registry["license_file"] = str(outside)
        tracked = core / "resources" / "licenses"
        tracked.mkdir(parents=True)
        (tracked / "MIT.txt").write_text("Tracked standard MIT text\n", encoding="utf-8")
        with patch.object(LICENSES, "TRACKED_LICENSES", tracked):
            components = self.collect([vendor, registry], root=core)
        self.assertEqual([component.name for component in components], ["zip", "registry"])
        self.assertEqual(
            [component.license_files[0].read_text() for component in components],
            ["Tracked standard MIT text\n", "Explicit registry text\n"],
        )


if __name__ == "__main__":
    unittest.main()
