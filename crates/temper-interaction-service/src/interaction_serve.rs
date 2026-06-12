//! Entrypoint glue for the generic interaction HTTP service.

use crate::interaction_args::InteractionServeArgs;
use crate::interaction_bindings::{
    build_service, load_bindings, load_compiled_spec, service_options, InteractionDeploymentError,
};
use crate::interaction_service::{run_http, InteractionHttpApp};

pub fn run_serve(args: &InteractionServeArgs) -> Result<(), InteractionDeploymentError> {
    let spec = load_compiled_spec(&args.spec_path)?;
    let bindings = load_bindings(&args.bindings_path)?;
    let options = service_options(
        &bindings,
        args.bind_override,
        args.allow_non_loopback,
        |key| std::env::var(key).ok(),
    )?;
    let runtime = temper_io_engine::build_runtime().map_err(InteractionDeploymentError::Config)?;
    let service = temper_io_engine::runtime::block_on_runtime_with(
        &runtime,
        move |cx, _handle| async move {
            build_service(&cx, &spec, &bindings, |key| std::env::var(key).ok())
        },
    )?;
    let app = InteractionHttpApp::new(service, options.service_token);
    run_http(options.bind, app, &runtime)?;
    Ok(())
}
