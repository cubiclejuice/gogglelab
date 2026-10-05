from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest


SCRIPT = Path(__file__).parents[1] / "scripts" / "package-release.py"
SPEC = importlib.util.spec_from_file_location("package_release", SCRIPT)
assert SPEC and SPEC.loader
package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(package)


class PackageReleaseTests(unittest.TestCase):
    def test_official_download_names_do_not_claim_a_separate_pro_edition(self) -> None:
        self.assertEqual(
            package.asset_base("official", "0.1.1", "macos", "arm64"),
            "gogglelab-0.1.1-macos-arm64",
        )

    def test_legacy_preview_names_remain_available_for_old_release_reproduction(self) -> None:
        self.assertEqual(
            package.asset_base("community", "0.1.0", "linux", "x64"),
            "gogglelab-community-0.1.0-linux-x64",
        )
        self.assertEqual(
            package.asset_base("pro", "0.1.0", "windows", "x64"),
            "gogglelab-pro-0.1.0-windows-x64",
        )

    def test_official_manifest_pins_private_build_and_public_core(self) -> None:
        assets = {"gogglelab-0.1.1-macos-arm64.zip": "f" * 64}
        manifest = package.manifest_payload(
            edition="official",
            version="0.1.1",
            target="aarch64-apple-darwin",
            platform="macos",
            arch="arm64",
            assets=assets,
            private_build_commit="a" * 40,
            public_core_commit="b" * 40,
        )
        self.assertEqual(manifest["application_identifier"], "com.sonicparke.stl-handoff")
        self.assertEqual(manifest["private_build_commit"], "a" * 40)
        self.assertEqual(manifest["public_core_commit"], "b" * 40)
        self.assertEqual(manifest["assets"], assets)

    def test_official_manifest_requires_both_exact_commit_pins(self) -> None:
        with self.assertRaisesRegex(ValueError, "private build commit"):
            package.manifest_payload(
                edition="official",
                version="0.1.1",
                target="aarch64-apple-darwin",
                platform="macos",
                arch="arm64",
                assets={},
                private_build_commit="short",
                public_core_commit="b" * 40,
            )


if __name__ == "__main__":
    unittest.main()
