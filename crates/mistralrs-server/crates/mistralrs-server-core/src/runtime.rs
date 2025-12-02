use std::future::Future;

use tokio::{runtime::Handle, task::JoinHandle};

#[derive(Clone)]
pub struct RuntimeAdapters {
    handle: Handle,
}

impl RuntimeAdapters {
    pub fn new(handle: Handle) -> Self {
        Self { handle }
    }

    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(func)
    }

    pub fn spawn_download<F>(&self, fut: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(fut)
    }
}

impl RuntimeAdapters {
    pub fn current() -> Self {
        Self {
            handle: Handle::current(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_blocking_executes_on_runtime() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let adapters = RuntimeAdapters::new(rt.handle().clone());
        let result = rt.block_on(async {
            let handle = adapters.spawn_blocking(|| 2 + 2);
            handle.await.expect("join blocking")
        });
        assert_eq!(result, 4);
    }

    #[test]
    fn spawn_download_runs_future() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let adapters = RuntimeAdapters::new(rt.handle().clone());
        let result = rt.block_on(async {
            let handle = adapters.spawn_download(async { 5usize });
            handle.await.expect("join download")
        });
        assert_eq!(result, 5);
    }
}
