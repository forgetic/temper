"""Artifact manifest, integrity inspection and copy transport. No DB merges."""
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

from contract import Refusal, file_digest, identity


def config_identity(config):
    run = config["experiment"]
    return identity({"mode": run["mode"], "environment": run["provider_environment"],
                     "auto_index": False, "auto_watch": False, "ui_enabled": False,
                     "reader_profile": "analysis", "mutation_profile": "all",
                     "path": "/usr/bin:/bin", "locale": "C.UTF-8", "external_git_config": "disabled"})


def decompress(archive, output, expected_size):
    if not isinstance(expected_size, int) or not 0 < expected_size <= 4 * 1024 ** 3:
        raise Refusal("artifact-decompressed-size")
    written = 0
    with output.open("wb") as sink:
        child = subprocess.Popen(["zstd", "-q", "-d", "-c", str(archive)], stdout=subprocess.PIPE,
                                 stderr=subprocess.DEVNULL)
        try:
            while block := child.stdout.read(1024 * 1024):
                written += len(block)
                if written > expected_size:
                    raise Refusal("artifact-decompressed-size")
                sink.write(block)
            if child.wait() != 0 or written != expected_size:
                raise Refusal("artifact-decompression-failed")
        finally:
            child.stdout.close()
            if child.poll() is None:
                child.kill()
            child.wait()


def inspect_database(path, project, expected_root):
    """Read actual SQLite contents; publish categories, never raw values."""
    db = sqlite3.connect(path.resolve().as_uri() + "?mode=ro&immutable=1", uri=True)
    try:
        if db.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
            raise Refusal("artifact-sqlite-integrity")
        tables = [row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")]
        projects = db.execute("SELECT name,root_path,indexed_at FROM projects").fetchall()
        matches = [row for row in projects if row[0] == project]
        if len(matches) != 1 or matches[0][1] != str(expected_root) or not matches[0][2]:
            raise Refusal("artifact-database-identity")
        absolute_paths = source_properties = text_values = 0
        for name in tables:
            quoted = '"' + name.replace('"', '""') + '"'
            for row in db.execute("SELECT * FROM " + quoted):
                for value in row:
                    if isinstance(value, str):
                        text_values += 1
                        absolute_paths += value.startswith("/") or '"/' in value
                        source_properties += any(key in value for key in ('"source"', '"signature"', '"docstring"'))
        return {"sqlite_integrity": True, "database_identity_confirmed": True,
                "table_count": len(tables), "text_value_count": text_values,
                "absolute_path_values": absolute_paths, "source_property_values": source_properties,
                "generation": matches[0][2]}
    except sqlite3.DatabaseError as error:
        raise Refusal("artifact-sqlite-invalid") from error
    finally:
        db.close()


def make_manifest(directory, config, producer_root):
    metadata = json.loads((directory / "artifact.json").read_text())
    if metadata.get("commit") != config["source"]["p0"] or metadata.get("project") != config["project"]:
        raise Refusal("export-metadata-identity")
    archive = directory / "graph.db.zst"
    if metadata.get("compressed_size") != archive.stat().st_size:
        raise Refusal("export-size-mismatch")
    with tempfile.TemporaryDirectory(prefix="inspect-", dir=directory.parent) as temporary:
        expanded = Path(temporary) / "graph.db"
        decompress(archive, expanded, metadata["original_size"])
        inspected = inspect_database(expanded, config["project"], producer_root)
    manifest = {"manifest_version": 1, "source_commit": config["source"]["p0"],
                "provider_version": config["provider"]["version"], "provider_sha256": config["provider"]["sha256"],
                "config_sha256": config_identity(config), "ignore_sha256": config["source"]["ignore_sha256"]["p0"],
                "schema_version": metadata["schema_version"], "artifact_sha256": file_digest(archive),
                "artifact_bytes": archive.stat().st_size, "metadata_sha256": file_digest(directory / "artifact.json"),
                "expanded_bytes": metadata["original_size"], "project": config["project"],
                "export_trigger": "explicit-index-persistence-true", "inspection": inspected}
    (directory / "manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    return manifest


def validate_artifact(directory, config, expected_manifest):
    """Immutable expected manifest is trusted; downloaded metadata is not."""
    required = [directory / p for p in ("graph.db.zst", "artifact.json", "manifest.json")]
    if any(not p.is_file() or p.is_symlink() for p in required):
        raise Refusal("artifact-missing")
    try:
        manifest = json.loads(required[2].read_text())
        metadata = json.loads(required[1].read_text())
    except (ValueError, OSError) as error:
        raise Refusal("artifact-metadata-corrupt") from error
    if manifest != expected_manifest:
        raise Refusal("artifact-manifest-mismatch")
    expected = {"source_commit": config["source"]["p0"], "provider_version": config["provider"]["version"],
                "provider_sha256": config["provider"]["sha256"], "config_sha256": config_identity(config),
                "ignore_sha256": config["source"]["ignore_sha256"]["p0"], "project": config["project"]}
    if any(manifest.get(k) != v for k, v in expected.items()):
        raise Refusal("artifact-identity-mismatch")
    if metadata.get("schema_version") != manifest["schema_version"]:
        raise Refusal("artifact-incompatible-schema")
    if metadata.get("commit") != manifest["source_commit"] or metadata.get("project") != manifest["project"]:
        raise Refusal("artifact-source-mismatch")
    if required[0].stat().st_size != manifest["artifact_bytes"]:
        raise Refusal("artifact-size-mismatch")
    if file_digest(required[0]) != manifest["artifact_sha256"]:
        raise Refusal("artifact-checksum-mismatch")
    if file_digest(required[1]) != manifest["metadata_sha256"]:
        raise Refusal("artifact-metadata-checksum-mismatch")
    return True


def transfer(source, destination, failure=None):
    started = time.monotonic()
    destination.mkdir()
    for name in ("graph.db.zst", "artifact.json", "manifest.json"):
        shutil.copyfile(source / name, destination / name)
    archive = destination / "graph.db.zst"
    if failure == "missing":
        archive.unlink()
    elif failure == "corrupt":
        data = bytearray(archive.read_bytes())
        data[len(data) // 2] ^= 255
        archive.write_bytes(data)
    elif failure == "truncated":
        with archive.open("r+b") as target:
            target.truncate(archive.stat().st_size // 2)
    elif failure == "incompatible":
        metadata = json.loads((destination / "artifact.json").read_text())
        metadata["schema_version"] += 10000
        (destination / "artifact.json").write_text(json.dumps(metadata))
    return {"transport": "local-filesystem-copy", "transfer_seconds": time.monotonic() - started,
            "transfer_bytes": sum(p.stat().st_size for p in destination.iterdir())}


def quarantine(directory):
    """Preserve failed bytes outside the consumer checkout; never delete caches."""
    if directory.exists():
        destination = directory.parent.parent / (directory.parent.name + "-quarantined-artifact")
        if destination.exists():
            raise Refusal("quarantine-already-exists")
        os.rename(directory, destination)
