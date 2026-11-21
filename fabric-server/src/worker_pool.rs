//! Bounded fixed-size worker pool for `fabric-server` TCP connection handling.

use std::sync::{Arc, Mutex, mpsc};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) struct ConnectionWorkerPool {
    sender: Option<mpsc::SyncSender<Job>>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl ConnectionWorkerPool {
    pub(crate) fn new(worker_count: usize, max_queued: usize) -> Result<Self, String> {
        let count = worker_count.clamp(1, 16);
        let (sender, receiver) = mpsc::sync_channel::<Job>(max_queued.max(count));
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::with_capacity(count);
        for idx in 0..count {
            let rx = Arc::clone(&receiver);
            let handle = thread::Builder::new()
                .name(format!("fabric-srv-{idx}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => break,
                        };
                        let Ok(job) = job else {
                            break;
                        };
                        job();
                    }
                })
                .map_err(|e| format!("failed to spawn server worker: {e}"))?;
            workers.push(handle);
        }
        Ok(Self {
            sender: Some(sender),
            workers,
        })
    }

    pub(crate) fn try_submit(&self, job: impl FnOnce() + Send + 'static) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|tx| tx.try_send(Box::new(job)).is_ok())
    }

    pub(crate) fn shutdown(mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for ConnectionWorkerPool {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
