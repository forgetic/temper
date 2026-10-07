# Building with local Smith

The typed system world uses Smith's domain crates from the Git revision
recorded in `Cargo.lock`. Until Smith is available at the forge, Cargo
resolves that revision from the local Smith repository. Source the local
redirect in the same shell as any Cargo command that resolves or builds
the new dependency:

```sh
. ~/src/rust/tmp/local-deps.env
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=~/src/rust/temper/target cargo check --workspace
```

The redirect changes only where Cargo fetches the revision. The manifest
and lockfile retain the forge URL. Smith and Temper must resolve the same
Skein revision.
