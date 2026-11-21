//! Bounded lazy worker pool for Spindle remote resolution and forwarding jobs.
//! Uses only `std` and `rustix::io::write` on an `eventfd` wakeup descriptor.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) struct WorkerPool {
    name_prefix: String,
    max_workers: usize,
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: Arc<Mutex<mpsc::Receiver<Job>>>,
    workers: Vec<thread::JoinHandle<()>>,
    wakeup: Arc<OwnedFd>,
}

impl WorkerPool {
    pub(crate) fn new(
        name_prefix: impl Into<String>,
        max_workers: usize,
        max_queued: usize,
        wakeup: Arc<OwnedFd>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(max_queued.max(1));
        Self {
            name_prefix: name_prefix.into(),
            max_workers: max_workers.max(1),
            sender: Some(sender),
            receiver: Arc::new(Mutex::new(receiver)),
            workers: Vec::new(),
            wakeup,
        }
    }

    pub(crate) fn try_submit(&mut self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        let Some(sender) = &self.sender else {
            return Err("spindle worker pool shutting down".into());
        };
        if self.workers.len() < self.max_workers {
            let id = self.workers.len();
            let receiver = Arc::clone(&self.receiver);
            let wakeup = Arc::clone(&self.wakeup);
            let handle = thread::Builder::new()
                .name(format!("{}-{id}", self.name_prefix))
                .spawn(move || {
                    loop {
                        let task = match receiver.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => break,
                        };
                        let Ok(task) = task else {
                            break;
                        };
                        task();
                        let _ = rustix::io::write(&*wakeup, &1u64.to_ne_bytes());
                    }
                })
                .map_err(|e| format!("worker spawn failed: {e}"))?;
            self.workers.push(handle);
        }
        sender
            .try_send(Box::new(job))
            .map_err(|_| "spindle worker pool saturated".to_owned())
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
