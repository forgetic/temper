/// Drives a single boxed future to completion on a one-shot engine runtime;
/// the real backend's futures park on network IO.
pub(super) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    temper_engine_io::build_runtime()
        .expect("engine runtime builds")
        .block_on(future)
}

/// [`block_on`] handing the body the root task's `Cx` (clock capability) for
/// code whose deadlines must be computed against the engine clock.
pub(super) fn block_on_with_cx<F, Fut>(f: F) -> Fut::Output
where
    F: FnOnce(temper_engine_io::Cx) -> Fut + Send + 'static,
    Fut: std::future::Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    let runtime = temper_engine_io::build_runtime().expect("engine runtime builds");
    temper_engine_io::runtime::block_on_runtime_with(&runtime, move |cx, _handle| f(cx))
}
