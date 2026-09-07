fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("usage: temper-benchmark-stack OUTPUT_DIR")?;
    temper_testing::benchmark_stack::run(std::path::Path::new(&output))
}
