#!/usr/bin/env python3

from pathlib import Path
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package.layout import build_package_dir
from codex_package.layout import validate_package_dir
from codex_package.targets import PACKAGE_VARIANTS
from codex_package.targets import TARGET_SPECS
from codex_package.targets import PackageInputs


class PackageLayoutTest(unittest.TestCase):
    def test_claudex_package_includes_codex_companion_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            inputs = PackageInputs(
                entrypoint_bin=touch_executable(root / "claudex"),
                companion_bins=(touch_executable(root / "codex"),),
                rg_bin=touch_executable(root / "rg"),
                bwrap_bin=touch_executable(root / "bwrap"),
                codex_command_runner_bin=None,
                codex_windows_sandbox_setup_bin=None,
            )
            package_dir = root / "package"
            package_dir.mkdir()

            build_package_dir(
                package_dir,
                "0.0.0-test",
                PACKAGE_VARIANTS["claudex"],
                TARGET_SPECS["x86_64-unknown-linux-musl"],
                inputs,
            )
            validate_package_dir(
                package_dir,
                PACKAGE_VARIANTS["claudex"],
                TARGET_SPECS["x86_64-unknown-linux-musl"],
            )

            self.assertTrue((package_dir / "bin" / "claudex").is_file())
            self.assertTrue((package_dir / "bin" / "codex").is_file())


def touch_executable(path: Path) -> Path:
    path.write_text("", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path.resolve()


if __name__ == "__main__":
    unittest.main()
