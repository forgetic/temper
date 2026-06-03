//! Cache-population helper for the Forgejo fixture binaries.
//!
//! The regular Forgejo ignored tests require the pinned binaries to already be
//! present under `.cache/forgejo/` (or supplied through `TEMPER_FORGEJO_BINARY`
//! / `TEMPER_FORGEJO_RUNNER_BINARY`) so those tests fail predictably when the
//! cache is absent. Run this ignored helper when a checkout needs the cache
//! populated from the pinned upstream release assets.

use temper_forgejo_fixture::download;

#[test]
#[ignore = "downloads pinned Forgejo + forgejo-runner binaries into .cache/forgejo"]
fn populate_forgejo_binary_cache() {
    let forgejo = download::ensure_binary().expect("Forgejo server binary downloads and verifies");
    let runner =
        download::ensure_runner_binary().expect("forgejo-runner binary downloads and verifies");
    eprintln!("Forgejo cached at {}", forgejo.display());
    eprintln!("forgejo-runner cached at {}", runner.display());
}
