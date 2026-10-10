//! One server owns its two workers even when its serving future is cancelled.
//! Cleanup starts before any subsequent await; Drop only signals it. Joining is
//! deliberately off the async executor and continues if finish is cancelled.
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

pub(crate) struct Workers {
    stop: Arc<AtomicBool>,
    http: axum_server::Handle<std::net::SocketAddr>,
    sealer: Option<mpsc::SyncSender<JoinHandle<()>>>,
    registered: bool,
    completion: Option<tokio::task::JoinHandle<io::Result<()>>>,
}

impl Workers {
    /// Must immediately follow successful commit-thread startup. The cleanup
    /// task is already scheduled, so Drop need not spawn during runtime teardown.
    pub(crate) fn new(
        commit: JoinHandle<()>,
        http: axum_server::Handle<std::net::SocketAddr>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<JoinHandle<()>>(1);
        let completion = tokio::task::spawn_blocking(move || {
            // No panic-producing worker join happens until the owner closes the
            // channel. Its single successful handoff therefore cannot race a
            // receiver exit. Startup failure may close it without a sealer.
            let sealer = receiver.recv().ok();
            while receiver.recv().is_ok() {}
            let sealer_panicked = sealer.is_some_and(|worker| worker.join().is_err());
            // Always drain/join commit, including after a sealer panic.
            let commit_panicked = commit.join().is_err();
            if sealer_panicked || commit_panicked {
                let error = io::Error::other(format!(
                    "server worker panic: sealer={sealer_panicked}, commit={commit_panicked}"
                ));
                // A dropped serving future cannot receive this error.
                eprintln!("fabric-server: lifecycle cleanup: {error}");
                Err(error)
            } else {
                Ok(())
            }
        });
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            http,
            sealer: Some(sender),
            registered: false,
            completion: Some(completion),
        }
    }

    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub(crate) fn register_sealer(&mut self, worker: JoinHandle<()>) {
        assert!(!self.registered, "a server owns exactly one sealer");
        self.sealer
            .as_ref()
            .expect("owner is open")
            .send(worker)
            .expect("cleanup receiver lives until owner closure");
        self.registered = true;
    }

    fn signal(&mut self) {
        self.http.shutdown();
        self.stop.store(true, Ordering::SeqCst);
        self.sealer.take();
    }

    pub(crate) async fn finish(mut self) -> io::Result<()> {
        self.signal();
        self.completion
            .take()
            .expect("cleanup exists")
            .await
            .map_err(|error| io::Error::other(format!("server cleanup task: {error}")))?
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.signal();
        // Dropping a Tokio JoinHandle detaches rather than cancels an already
        // scheduled blocking cleanup. It retains both std worker handles.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_owner_without_sealer_joins_commit() {
        let (release, receive) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let commit = std::thread::spawn(move || {
            receive.recv().unwrap();
            done.send(()).unwrap();
        });
        let owner = Workers::new(commit, axum_server::Handle::new());
        let stop = owner.stop_flag();
        drop(owner); // The sealer-spawn-failure path.
        let stopped = stop.load(Ordering::SeqCst);
        release.send(()).unwrap();
        tokio::task::block_in_place(|| finished.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(stopped);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_finish_keeps_cleanup_and_both_workers_owned() {
        let (release, receive) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let commit = std::thread::spawn(move || {
            receive.recv().unwrap();
            done.send(()).unwrap();
        });
        let mut owner = Workers::new(commit, axum_server::Handle::new());
        let stop = owner.stop_flag();
        let (entered, joining) = tokio::sync::oneshot::channel();
        let (allow_exit, await_exit) = mpsc::channel();
        owner.register_sealer(std::thread::spawn(move || {
            entered.send(()).unwrap();
            await_exit.recv().unwrap();
            release.send(()).unwrap();
        }));
        joining.await.unwrap();
        let finishing = tokio::spawn(owner.finish());
        let stopped = tokio::time::timeout(Duration::from_secs(2), async {
            while !stop.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await;
        finishing.abort();
        let cancelled = finishing.await;
        allow_exit.send(()).unwrap();
        tokio::task::block_in_place(|| finished.recv_timeout(Duration::from_secs(2)).unwrap());
        stopped.unwrap();
        assert!(cancelled.unwrap_err().is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sealer_panic_is_reported_after_commit_is_joined() {
        let (release, receive) = mpsc::channel();
        let commit = std::thread::spawn(move || {
            receive.recv().unwrap();
        });
        let mut owner = Workers::new(commit, axum_server::Handle::new());
        owner.register_sealer(std::thread::spawn(move || {
            release.send(()).unwrap();
            panic!("injected outer sealer panic");
        }));
        let error = tokio::time::timeout(Duration::from_secs(2), owner.finish())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("sealer=true, commit=false"));
    }
}
