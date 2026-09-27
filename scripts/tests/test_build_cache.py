"""Exercise the installed cache across checkout paths and garbage collection."""

import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest import mock


KACHE = os.environ.get("KACHE_TEST_BINARY") or shutil.which("kache")


class CachePreparationTests(unittest.TestCase):
    def test_retains_warm_targets_and_invalidates_them_once_per_upgrade(self):
        prepare = runpy.run_path(
            str(Path(__file__).resolve().parents[1] / "prepare-build-cache.py")
        )["prepare"]
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "configured-target"
            target.mkdir()
            artifact = target / "retained-artifact"
            artifact.write_text("old wrapper output")
            version = ["kache 0.26.3"]

            def output(command, **kwargs):
                if command == ["kache", "--version"]:
                    return version[0]
                return json.dumps({"target_directory": str(target)})

            def clean(command, **kwargs):
                self.assertEqual(command, ["cargo", "clean", "--target-dir", str(target)])
                shutil.rmtree(target)

            with (
                mock.patch.dict(os.environ, {"RUSTC_WRAPPER": "kache"}),
                mock.patch("shutil.which", return_value="kache"),
                mock.patch("subprocess.check_output", side_effect=output),
                mock.patch("subprocess.run", side_effect=clean) as cleaner,
            ):
                prepare()
                self.assertFalse(artifact.exists())
                artifact.write_text("warm output")
                prepare()
                self.assertEqual(artifact.read_text(), "warm output")
                self.assertEqual(cleaner.call_count, 1)
                version[0] = "kache 0.27.0"
                prepare()
                self.assertFalse(artifact.exists())
                self.assertEqual(cleaner.call_count, 2)
                self.assertEqual((target / ".temper-kache-version").read_text().strip(), version[0])


@unittest.skipUnless(KACHE and shutil.which("cargo"), "requires cargo and kache")
class BuildCacheTests(unittest.TestCase):
    def test_gc_preserves_shared_outputs_and_restores_custom_harness(self):
        with tempfile.TemporaryDirectory(prefix="temper-cache-") as temporary:
            root = Path(temporary)
            config = root / "kache.toml"
            config.write_text("[cache]\ncache_executables = true\n")
            env = os.environ | {
                "RUSTC_WRAPPER": KACHE,
                "RUSTC_WORKSPACE_WRAPPER": "",
                "KACHE_CONFIG": str(config),
                "KACHE_HOST_CONFIG": "",
                "KACHE_CACHE_DIR": str(root / "cache"),
                "KACHE_RUNTIME_DIR": str(root / "cache"),
                "KACHE_MAX_SIZE": "1GiB",
                "KACHE_AUTO_GC": "0",
                "KACHE_SCHEDULER": "0",
                "KACHE_LOCAL_ONLY": "1",
                "KACHE_KEY_SALT": "producer",
            }
            # Separate keys can legitimately share all output blobs. Removing
            # one key saves no artifact storage, but destroys a future hit.
            self.build(root / "producer", env)
            time.sleep(1.1)  # GC access timestamps have second precision.
            shutil.rmtree(root / "producer/target")
            self.build(root / "producer", env | {"KACHE_KEY_SALT": "other"})
            self.assertGreater(self.report(root, env)["dups"], 0)
            self.run_command([KACHE, "gc"], root, env)
            before = self.report(root, env)

            executables = self.build(root / "ci", env)
            after = self.report(root, env)
            self.assertGreater(after["local_hits"], before["local_hits"])
            for counter in ("misses", "dups", "errors", "store_failures", "passthroughs"):
                self.assertEqual(after[counter], before[counter], counter)
            custom = next(path for name, path in executables if name == "custom")
            result = self.run_command([custom], root / "ci", env)
            self.assertEqual(result.stdout.strip(), "custom harness ran")

    def build(self, checkout, env):
        (checkout / "src").mkdir(parents=True, exist_ok=True)
        (checkout / "tests").mkdir(exist_ok=True)
        (checkout / "Cargo.toml").write_text(
            '[package]\nname = "cache-contract"\nversion = "0.1.0"\n'
            'edition = "2021"\n[[test]]\nname = "custom"\n'
            'path = "tests/custom.rs"\nharness = false\n'
        )
        (checkout / "src/lib.rs").write_text(
            "pub fn answer() -> u32 { 42 }\n"
            "#[test] fn check() { assert_eq!(answer(), 42); }\n"
        )
        (checkout / "tests/custom.rs").write_text(
            'fn main() { assert_eq!(cache_contract::answer(), 42); '
            'println!("custom harness ran"); }\n'
        )
        result = self.run_command(
            ["cargo", "test", "--all-targets", "--no-run", "--message-format=json"],
            checkout,
            env | {"CARGO_TARGET_DIR": str(checkout / "target")},
        )
        messages = [json.loads(line) for line in result.stdout.splitlines()]
        return [
            (message["target"]["name"], message["executable"])
            for message in messages
            if message.get("executable")
        ]

    def report(self, root, env):
        result = self.run_command(
            [KACHE, "report", "--format", "json", "--since", "1h"], root, env
        )
        return json.loads(result.stdout)["summary"]

    def run_command(self, command, cwd, env):
        result = subprocess.run(
            command, cwd=cwd, env=env, text=True, capture_output=True, timeout=60
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result
