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

    def package(self, name, directory, *, source=None, repository=None):
        directory.mkdir(parents=True)
        manifest = directory / "Cargo.toml"
        manifest.write_text("# Synthetic metadata fixture\n", encoding="utf-8")
        (directory / "LICENSE").write_text(f"Synthetic license for {name}\n", encoding="utf-8")
        return {
            "name": name,
            "version": "1.0.0",
            "manifest_path": str(manifest),
            "license": "MIT",
            "source": source,
            "repository": repository,
        }

    def collect(self, packages):
        metadata = SimpleNamespace(stdout=json.dumps({"packages": packages}))
        with patch.object(LICENSES, "PROJECT", self.project), patch.object(
            LICENSES.subprocess, "run", return_value=metadata
        ) as run:
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
        self.assertEqual(components[0].license_files, (self.project / "vendor" / "zip" / "LICENSE",))
        self.assertEqual(components[0].license_files[0].read_text(), "Synthetic license for zip\n")

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


if __name__ == "__main__":
    unittest.main()
