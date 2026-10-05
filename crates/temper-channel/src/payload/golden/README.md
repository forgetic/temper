# V2 payload golden fixtures

The 181 `.bin` files here are checked-in codec regression expectations.
Each has an explicit Rust value in `../tests_v2.rs`; the test compares fresh
encoding with the file, then checks decoding and truncated input. The
fixture manifest names those same test functions and verifies that its
entries, invoked filenames and binary files agree exactly. Missing files,
stale files and missing manifest coverage fail normal tests.

From the repository root, regenerate **both** this directory and the
[wire fixtures](../../wire/golden/README.md) (37 binaries) with:

```sh
cargo test -p temper-channel regenerate_goldens -- --ignored
```

This opt-in command invokes the existing explicit-value test functions in
write mode, encodes their values using the current codec, writes each file,
and checks the freshly written bytes. It also recreates missing files. It
does not need the old files to compile or their bytes to decode, and it does
not remove stale binaries automatically. Ordinary tests never write files;
regeneration mode is an explicit argument passed only by the ignored tests.

After an intended schema or fixture-value change, regenerate, inspect the
binary diff against the channel design, then run the normal checks:

```sh
cargo test -p temper-channel tests_v2
```

Do not regenerate merely to silence unexpected codec drift. Generated
fixtures and codec round trips are regression checks, not independent
proof of wire compatibility. The separate literal byte arrays in
`../tests_v2.rs` check selected field positions independently and are never
rewritten by regeneration. Regeneration adds no protocol-world or captured
peer evidence for later migration stages.
