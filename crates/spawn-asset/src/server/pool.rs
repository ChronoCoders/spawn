//! IO thread pool: a fixed set of `std::thread` workers draining a **bounded**
//! job queue. Each worker reads a file, dispatches to the matching loader by
//! extension, and sends the result over the completion channel. Workers never
//! touch shared asset state; all slot mutation happens on the main thread in
//! `apply_loaded`.
//!
//! Backpressure: the job queue is bounded. The server treats a full queue as
//! deferred-pending (retried next pump), never blocking the caller and never
//! dropping a request.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;

use crate::error::AssetError;
use crate::id::AssetId;
use crate::loader::{ErasedLoader, ErasedPayload, LoadContext};

pub(crate) struct LoadJob {
    pub(crate) id: AssetId,
    pub(crate) abs_path: PathBuf,
    pub(crate) canonical_path: String,
    pub(crate) extension: String,
    pub(crate) is_reload: bool,
}

pub(crate) enum JobResult {
    Loaded {
        id: AssetId,
        payload: ErasedPayload,
        is_reload: bool,
    },
    Failed {
        id: AssetId,
        error: AssetError,
        is_reload: bool,
    },
}

pub(crate) struct IoPool {
    job_tx: Option<SyncSender<LoadJob>>,
    result_rx: std::sync::Mutex<Receiver<JobResult>>,
    workers: Vec<JoinHandle<()>>,
}

pub(crate) type LoaderTable = Arc<RwLock<HashMap<String, Arc<dyn ErasedLoader>>>>;

impl IoPool {
    pub(crate) fn new(threads: usize, capacity: usize, loaders: LoaderTable) -> Self {
        let threads = threads.max(1);
        let capacity = capacity.max(1);
        let (job_tx, job_rx) = sync_channel::<LoadJob>(capacity);
        let (result_tx, result_rx) = std::sync::mpsc::channel::<JobResult>();
        let job_rx = Arc::new(std::sync::Mutex::new(job_rx));

        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            let job_rx = Arc::clone(&job_rx);
            let result_tx = result_tx.clone();
            let loaders = Arc::clone(&loaders);
            let handle = std::thread::spawn(move || {
                worker_loop(&job_rx, &result_tx, &loaders);
            });
            workers.push(handle);
        }

        Self {
            job_tx: Some(job_tx),
            result_rx: std::sync::Mutex::new(result_rx),
            workers,
        }
    }

    /// Attempts to enqueue a job without blocking. Returns the job back if the
    /// bounded queue is full so the caller can retry on the next pump.
    pub(crate) fn try_submit(&self, job: LoadJob) -> Result<(), LoadJob> {
        match &self.job_tx {
            Some(tx) => match tx.try_send(job) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(job)) => Err(job),
                Err(TrySendError::Disconnected(job)) => Err(job),
            },
            None => Err(job),
        }
    }

    pub(crate) fn drain_results(&self) -> Vec<JobResult> {
        let mut out = Vec::new();
        let rx = match self.result_rx.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        while let Ok(result) = rx.try_recv() {
            out.push(result);
        }
        out
    }
}

impl Drop for IoPool {
    fn drop(&mut self) {
        // Dropping the sender closes the queue; workers observe the channel
        // disconnect, exit their loops, and are joined here. No detached
        // threads survive the pool.
        self.job_tx = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    job_rx: &std::sync::Mutex<Receiver<LoadJob>>,
    result_tx: &std::sync::mpsc::Sender<JobResult>,
    loaders: &LoaderTable,
) {
    loop {
        let job = {
            let guard = match job_rx.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            match guard.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        let result = run_job(job, loaders);
        if result_tx.send(result).is_err() {
            return;
        }
    }
}

fn run_job(job: LoadJob, loaders: &LoaderTable) -> JobResult {
    let loader = {
        let table = match loaders.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match table.get(&job.extension) {
            Some(loader) => Arc::clone(loader),
            None => {
                return JobResult::Failed {
                    id: job.id,
                    error: AssetError::NoLoader {
                        extension: job.extension,
                    },
                    is_reload: job.is_reload,
                };
            }
        }
    };

    let bytes = match std::fs::read(&job.abs_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let error = if err.kind() == std::io::ErrorKind::NotFound {
                AssetError::NotFound {
                    path: job.canonical_path,
                }
            } else {
                AssetError::Io {
                    path: job.canonical_path,
                    kind: err.kind(),
                }
            };
            return JobResult::Failed {
                id: job.id,
                error,
                is_reload: job.is_reload,
            };
        }
    };

    let ctx = LoadContext {
        id: job.id,
        canonical_path: &job.canonical_path,
        extension: &job.extension,
    };
    match loader.load_erased(&bytes, &ctx) {
        Ok(payload) => JobResult::Loaded {
            id: job.id,
            payload,
            is_reload: job.is_reload,
        },
        Err(error) => JobResult::Failed {
            id: job.id,
            error,
            is_reload: job.is_reload,
        },
    }
}
