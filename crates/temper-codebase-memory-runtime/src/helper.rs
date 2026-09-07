use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use crate::config::{BootstrapReply, HELPER_MODE, MAX_CONTROL_BYTES};
use crate::frontend::BootstrapFrontend;
use crate::{LaunchConfig, ProviderBootstrapFailure};

/// Dispatch only in a fresh helper process, before runtimes/threads start.
/// Parent configuration crosses an authenticated, bounded local control socket.
#[doc(hidden)]
pub fn dispatch_provider_bootstrap_helper(
    args: impl IntoIterator<Item = OsString>,
) -> Option<ExitCode> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_MODE)) {
        return None;
    }
    let result = (|| {
        if args.next().is_some() {
            return Err(ProviderBootstrapFailure::Configuration);
        }
        #[cfg(target_os = "linux")]
        return temper_process_containment::run_linux_transient_descendant_owner(run)
            .map_err(|_| ProviderBootstrapFailure::Spawn)?;
        #[cfg(not(target_os = "linux"))]
        Err(ProviderBootstrapFailure::UnsupportedPlatform)
    })();
    Some(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run() -> Result<(), ProviderBootstrapFailure> {
    let descriptor = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    let stream = UnixStream::from(descriptor);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_CONTROL_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    if bytes.len() > MAX_CONTROL_BYTES || bytes.last() != Some(&b'\n') {
        return Err(ProviderBootstrapFailure::Configuration);
    }
    let config: LaunchConfig =
        serde_json::from_slice(&bytes).map_err(|_| ProviderBootstrapFailure::Configuration)?;
    config.validate()?;
    let mut frontend = BootstrapFrontend::spawn(&config)?;
    let initialized = frontend.initialize(config.startup_timeout);
    let reply = match initialized {
        Ok(()) => BootstrapReply::Admitted,
        Err(category) => BootstrapReply::Failed { category },
    };
    let mut bytes = serde_json::to_vec(&reply).map_err(|_| ProviderBootstrapFailure::Transport)?;
    bytes.push(b'\n');
    reader
        .get_mut()
        .write_all(&bytes)
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    initialized?;
    reader
        .get_mut()
        .set_read_timeout(None)
        .map_err(|_| ProviderBootstrapFailure::Transport)?;
    let mut release = [0];
    // The parent enforces the finite admission deadline by cancelling and
    // joining its job FIRST, then closing this bootstrap channel.
    let _ = reader.read(&mut release);
    drop(frontend);
    Ok(())
}
