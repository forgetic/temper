#!/usr/bin/env python3
"""Check native test headers without executing the test harnesses."""

import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import sys


LIST_COMMAND = [
    "cargo", "nextest", "list", "--workspace", "--list-type", "binaries-only",
    "--message-format", "json",
]


def binary_paths(inventory):
    binaries = inventory.get("rust-binaries") if isinstance(inventory, dict) else None
    if not isinstance(binaries, dict) or not binaries:
        raise ValueError("expected a nonempty nextest rust-binaries inventory")
    paths = []
    for name, binary in binaries.items():
        path = binary.get("binary-path") if isinstance(binary, dict) else None
        if not isinstance(path, str) or not path or not Path(path).is_absolute():
            raise ValueError(f"invalid binary-path for {name!r}")
        paths.append(Path(path))
    return sorted(set(paths))


def elf_error(header, size):
    if header[:4] != b"\x7fELF":
        return "missing ELF signature"
    if len(header) < 16 or header[4] not in (1, 2) or header[5] not in (1, 2):
        return "invalid ELF identification"
    width = header[4]
    minimum = 52 if width == 1 else 64
    if len(header) < minimum or size < minimum:
        return "truncated ELF header"
    order = "little" if header[5] == 1 else "big"
    number = lambda start, end: int.from_bytes(header[start:end], order)
    header_size_offset = 40 if width == 1 else 52
    if (
        header[6] != 1
        or number(16, 18) not in (2, 3)
        or number(20, 24) != 1
        or number(header_size_offset, header_size_offset + 2) != minimum
    ):
        return "invalid ELF executable header"
    program_offset = number(28, 32) if width == 1 else number(32, 40)
    section_offset = number(32, 36) if width == 1 else number(40, 48)
    table = header_size_offset + 2
    program_size, program_count = number(table, table + 2), number(table + 2, table + 4)
    section_size, section_count = number(table + 4, table + 6), number(table + 6, table + 8)
    if (
        not program_count
        or program_size != (32 if width == 1 else 56)
        or program_offset < minimum
        or program_offset + program_size * program_count > size
        or section_count and (section_offset < minimum or not section_size
                              or section_offset + section_size * section_count > size)
    ):
        return "truncated or invalid ELF header tables"
    return None


def header_error(header, size, platform=sys.platform):
    if platform == "darwin":
        lengths = {
            b"\xfe\xed\xfa\xce": 28, b"\xce\xfa\xed\xfe": 28,
            b"\xfe\xed\xfa\xcf": 32, b"\xcf\xfa\xed\xfe": 32,
            b"\xca\xfe\xba\xbe": 8, b"\xbe\xba\xfe\xca": 8,
            b"\xca\xfe\xba\xbf": 8, b"\xbf\xba\xfe\xca": 8,
        }
        minimum = lengths.get(header[:4])
        return None if minimum and size >= minimum else "invalid Mach-O header"
    if platform == "win32":
        return None if header[:2] == b"MZ" and size >= 64 else "invalid PE header"
    return elf_error(header, size)


def check_binary(path):
    try:
        if not stat.S_ISREG(path.stat().st_mode):
            return "not a regular file"
        with path.open("rb") as binary:
            metadata = os.fstat(binary.fileno())
            if not stat.S_ISREG(metadata.st_mode):
                return "not a regular file"
            if not metadata.st_size:
                return "empty executable"
            if os.name == "posix" and not metadata.st_mode & 0o111:
                return "executable permission is missing"
            return header_error(binary.read(64), metadata.st_size)
    except OSError as error:
        return str(error)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, help="check a saved binaries-only JSON inventory")
    args = parser.parse_args(argv)
    try:
        if args.inventory:
            raw = args.inventory.read_text()
        else:
            result = subprocess.run(LIST_COMMAND, stdout=subprocess.PIPE, text=True)
            if result.returncode:
                return result.returncode if result.returncode > 0 else 1
            raw = result.stdout
        paths = binary_paths(json.loads(raw))
    except (OSError, ValueError) as error:
        print(f"Test executable inventory failed: {error}", file=sys.stderr)
        return 1
    failures = [(path, error) for path in paths if (error := check_binary(path))]
    if failures:
        for path, error in failures:
            print(f"Invalid test executable: {path}: {error}", file=sys.stderr)
        print("Rebuild the damaged outputs before running nextest; no files were changed.", file=sys.stderr)
        return 1
    print(f"OK - checked {len(paths)} native test executable header(s) without running them.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
