"""A damaged retained executable must not silently remove a test suite."""

import contextlib
import io
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "check-test-executables.py"
SPEC = importlib.util.spec_from_file_location("test_executable_guard", SCRIPT)
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class HeaderTests(unittest.TestCase):
    def test_running_python_is_a_valid_native_executable(self):
        self.assertIsNone(GUARD.check_binary(Path(sys.executable)))

    def test_missing_empty_truncated_zero_filled_and_non_executable_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "test-binary"
            self.assertIsNotNone(GUARD.check_binary(path))
            for body in [b"", b"\x7fELF", b"\0" * 4096, b"#!/bin/sh\nexit 0\n"]:
                path.write_bytes(body)
                path.chmod(0o755)
                self.assertIsNotNone(GUARD.check_binary(path), repr(body[:16]))
            shutil.copyfile(sys.executable, path)
            path.chmod(0o644)
            if os.name == "posix":
                self.assertIsNotNone(GUARD.check_binary(path))

    def test_invalid_elf_class_version_and_header_length(self):
        if sys.platform != "linux":
            self.skipTest("ELF regression")
        with open(sys.executable, "rb") as executable:
            header = bytearray(executable.read(64))
        size = Path(sys.executable).stat().st_size
        self.assertIsNone(GUARD.elf_error(header, size))
        self.assertIsNotNone(GUARD.elf_error(header, 64))
        size_offset = 40 if header[4] == 1 else 52
        for offset, value in [(4, 0), (5, 0), (6, 0), (16, 0), (20, 0), (size_offset, 0)]:
            damaged = header.copy()
            damaged[offset] = value
            self.assertIsNotNone(GUARD.elf_error(damaged, size), offset)

    def test_inventory_requires_binaries_only_schema_and_absolute_paths(self):
        for inventory in [None, [], {}, {"rust-binaries": {}}, {"rust-suites": {}},
                          {"rust-binaries": {"test": {"binary-path": "relative"}}}]:
            with self.assertRaises(ValueError):
                GUARD.binary_paths(inventory)

    def test_cargo_failure_is_preserved_without_reading_an_inventory(self):
        with patch.object(GUARD.subprocess, "run", return_value=subprocess.CompletedProcess([], 17, "")):
            self.assertEqual(GUARD.main([]), 17)

    def test_invalid_json_fails_closed(self):
        with contextlib.redirect_stderr(io.StringIO()), patch.object(
            GUARD.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "{")
        ):
            self.assertEqual(GUARD.main([]), 1)


class RetainedOutputRegression(unittest.TestCase):
    def test_real_harness_cannot_disappear_after_zero_fill(self):
        if sys.platform != "linux":
            self.skipTest("Linux nextest zero-filled executable regression")
        with tempfile.TemporaryDirectory(prefix="temper-test-header-") as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "test-header-fixture"\nversion = "0.0.0"\n'
                'edition = "2024"\n[workspace]\n'
                '[[test]]\nname = "empty"\npath = "empty.rs"\nharness = false\n'
            )
            (root / "src/lib.rs").write_text('#[test]\nfn counted() { assert_eq!(2 + 2, 4); }\n')
            (root / "empty.rs").write_text(
                'fn main() { std::fs::write("harness-executed", "yes").unwrap(); }\n'
            )
            # Fault injection must stay in this fixture. Its custom empty
            # harness also avoids the separate kache 0.11 permission issue.
            environment = os.environ.copy() | {
                "CARGO_TARGET_DIR": str(root / "target"), "RUSTC_WRAPPER": "",
            }

            def cargo(*arguments):
                result = subprocess.run(
                    ["cargo", "nextest", "list", "--workspace", *arguments, "--message-format", "json"],
                    cwd=root, env=environment, text=True, capture_output=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                return json.loads(result.stdout)

            inventory = cargo("--list-type", "binaries-only")
            paths = GUARD.binary_paths(inventory)
            self.assertEqual(len(paths), 2)
            errors = {str(path): GUARD.check_binary(path) for path in paths}
            self.assertTrue(all(error is None for error in errors.values()), errors)
            marker = root / "harness-executed"
            checked = subprocess.run(
                [sys.executable, str(SCRIPT)], cwd=root, env=environment,
                text=True, capture_output=True,
            )
            self.assertEqual(checked.returncode, 0, checked.stderr)
            self.assertFalse(marker.exists(), "binaries-only guard must not execute harnesses")
            binary = next(Path(item["binary-path"]) for item in inventory["rust-binaries"].values()
                          if item["kind"] == "lib")
            full = cargo()
            self.assertEqual(sum(len(suite["testcases"]) for suite in full["rust-suites"].values()), 1)
            self.assertTrue(marker.exists(), "full listing does execute the valid empty harness")
            marker.unlink()
            size = binary.stat().st_size
            damaged = binary.with_name(binary.name + ".damaged")
            damaged.write_bytes(b"\0" * size)
            damaged.chmod(binary.stat().st_mode)
            damaged.replace(binary)
            self.assertEqual(binary.stat().st_size, size)
            # The damaged executable returns an empty listing on the affected
            # nextest path. Its binary inventory still contains the output.
            full = cargo()
            self.assertEqual(sum(len(suite["testcases"]) for suite in full["rust-suites"].values()), 0)
            self.assertEqual(GUARD.binary_paths(cargo("--list-type", "binaries-only")), paths)
            self.assertIsNotNone(GUARD.check_binary(binary))
            marker.unlink()
            checked = subprocess.run(
                [sys.executable, str(SCRIPT)], cwd=root, env=environment,
                text=True, capture_output=True,
            )
            self.assertEqual(checked.returncode, 1, checked.stderr)
            self.assertIn(str(binary), checked.stderr)
            self.assertFalse(marker.exists(), "failed guard must not execute any harness")


if __name__ == "__main__":
    unittest.main()
