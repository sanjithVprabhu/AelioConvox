//! Tiny helper to drive async `object_store` from the sync Database API.

use std::future::Future;

/// Block on an async future from sync code.
///
/// Prefer the current Tokio runtime (aelio-db-api's axum runtime) via
/// `block_in_place` so we don't nest runtimes. Fall back to a fresh
/// current-thread runtime for unit tests / sync CLIs.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("create tokio runtime for object_store IO");
            rt.block_on(fut)
        }
    }
}
