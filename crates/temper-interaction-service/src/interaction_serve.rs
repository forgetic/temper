//! Entrypoint glue for the generic interaction HTTP service.

use crate::interaction_args::InteractionServeArgs;
use crate::interaction_bindings::{
    InteractionDeploymentError, build_service, load_bindings, load_compiled_spec, service_options,
};
use crate::interaction_service::{InteractionHttpApp, run_http};

pub fn run_serve(args: &InteractionServeArgs) -> Result<(), InteractionDeploymentError> {
    let spec = load_compiled_spec(&args.spec_path)?;
    let bindings = load_bindings(&args.bindings_path)?;
    let options = service_options(
        &bindings,
        args.bind_override,
        args.allow_non_loopback,
        |key| std::env::var(key).ok(),
    )?;
    let runtime = temper_engine_io::build_runtime().map_err(InteractionDeploymentError::Config)?;
    let service = temper_engine_io::runtime::block_on_runtime_with(
        &runtime,
        move |cx, _handle| async move {
            build_service(&cx, &spec, &bindings, |key| std::env::var(key).ok())
        },
    )?;
    let app = InteractionHttpApp::new(service, options.service_token);
    run_http(options.bind, app, &runtime)?;
    Ok(())
}
