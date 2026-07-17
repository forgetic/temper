use std::io;

use crate::{
    ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentSpec,
    PreparedContainmentBackend,
};

/// Explicit fail-closed backend for targets without a descendant-complete
/// platform primitive.
///
/// Production composition roots can install this factory on unsupported
/// targets and receive a preparation error before any payload is spawned.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedPlatformBackendFactory;

impl ContainmentBackendFactory for UnsupportedPlatformBackendFactory {
    fn prepare_backend(
        &self,
        _policy: ContainmentBackendPolicy,
        _spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "descendant-complete process containment is unsupported on {}",
                std::env::consts::OS
            ),
        ))
    }
}
