fn main() {
    if let Err(error) = resolve() {
        eprintln!("failed to resolve the pinned Bench Forgejo fixture: {error}");
        eprintln!(
            "check BENCH_FORGEJO_* overrides, network access, and the workspace .cache/forgejo directory"
        );
        std::process::exit(1);
    }
}

fn resolve() -> Result<(), Box<dyn std::error::Error>> {
    let forgejo = bench_forgejo::download::ensure_binary()?;
    let runner = bench_forgejo::download::ensure_runner_binary()?;

    println!(
        "forgejo_version={}",
        bench_forgejo::download::FORGEJO_VERSION
    );
    println!(
        "forgejo_runner_version={}",
        bench_forgejo::download::FORGEJO_RUNNER_VERSION
    );
    println!("forgejo={}", forgejo.display());
    println!("forgejo_runner={}", runner.display());
    Ok(())
}
