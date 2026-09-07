//! Bounded native test control, including panic-path fallback cleanup.
use super::*;
use std::io::Read;
use std::process::Stdio;

pub(super) fn native_control(
    runtime: &ProviderRuntime,
    action: &str,
) -> std::result::Result<String, &'static str> {
    let path = runtime
        .directory
        .path()
        .join(format!("control-{}.log", uuid::Uuid::new_v4()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "native control capture")?;
    // A private regular capture file avoids waiting for pipe EOF from a native
    // descendant if the controlling command itself stalls or exits early.
    let result = (|| {
        let mut child = Command::new("env")
            .args(&runtime.args)
            .args(["daemon", action])
            .stdout(Stdio::from(file))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "native control spawn")?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < Duration::from_secs(15) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("native control deadline or wait");
                }
            }
        }
        let mut output = String::new();
        fs::File::open(&path)
            .map_err(|_| "native control capture")?
            .take(16_385)
            .read_to_string(&mut output)
            .map_err(|_| "native control output")?;
        if output.len() > 16_384 {
            return Err("native control output bound");
        }
        Ok(output)
    })();
    let _ = fs::remove_file(path);
    result
}
