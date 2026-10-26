//! CPU jobs only. No platform renderer, game state, preferences or file I/O.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadBudget {
    pub logical_cpus: usize,
    pub reserved: usize,
    pub physics: usize,
    pub lighting: usize,
}
impl ThreadBudget {
    pub fn detected() -> Self {
        Self::for_cpus(thread::available_parallelism().map_or(4, usize::from), 1, 1)
    }
    /// Reserve host/render and game lanes. Weights divide the remaining heavy work.
    /// On four CPUs this gives one physics and one lighting owner, with no helpers.
    pub fn for_cpus(cpus: usize, physics_weight: u32, lighting_weight: u32) -> Self {
        let cpus = cpus.max(1);
        let reserved = 2.min(cpus.saturating_sub(2));
        let heavy = cpus.saturating_sub(reserved).max(2);
        let sum = u64::from(physics_weight.max(1)) + u64::from(lighting_weight.max(1));
        let physics =
            ((heavy as u64 * u64::from(physics_weight.max(1)) / sum) as usize).clamp(1, heavy - 1);
        Self {
            logical_cpus: cpus,
            reserved,
            physics,
            lighting: heavy - physics,
        }
    }
    pub fn helpers(self) -> usize {
        self.physics.saturating_sub(1) + self.lighting.saturating_sub(1)
    }
}
type Job = Box<dyn FnOnce() + Send + 'static>;
struct Queue {
    jobs: VecDeque<Job>,
    stopped: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}
pub struct TaskPool {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    helpers: usize,
}
impl TaskPool {
    pub fn new(helpers: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue {
                    jobs: VecDeque::new(),
                    stopped: false,
                }),
                ready: Condvar::new(),
            }),
            workers: Mutex::new(Vec::new()),
            helpers,
        }
    }
    pub fn spawned_helpers(&self) -> usize {
        self.workers.lock().unwrap().len()
    }
    pub fn shutdown(&self) {
        self.shared.queue.lock().unwrap().stopped = true;
        self.shared.ready.notify_all();
        for handle in self.workers.lock().unwrap().drain(..) {
            if handle.thread().id() != thread::current().id() {
                let _ = handle.join();
            }
        }
    }
    fn grow(&self, desired: usize) {
        let mut handles = self.workers.lock().unwrap();
        while handles.len() < desired.min(self.helpers) {
            let shared = self.shared.clone();
            let name = format!("charon-cpu-{}", handles.len());
            let worker = thread::Builder::new().name(name).spawn(move || {
                loop {
                    let job = {
                        let mut q = shared.queue.lock().unwrap();
                        while q.jobs.is_empty() && !q.stopped {
                            q = shared.ready.wait(q).unwrap();
                        }
                        if q.stopped && q.jobs.is_empty() {
                            return;
                        }
                        q.jobs.pop_front()
                    };
                    if let Some(job) = job {
                        job();
                    }
                }
            });
            match worker {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
    }
    fn help(&self) -> bool {
        let job = self.shared.queue.lock().unwrap().jobs.pop_front();
        if let Some(job) = job {
            job();
            true
        } else {
            false
        }
    }
    /// Ordered results, bounded queue, reusable threads. The caller participates;
    /// nested jobs make progress because a waiting caller also drains queued work.
    pub fn map<I: Send + 'static, O: Send + 'static, F: Fn(I) -> O + Send + Sync + 'static>(
        &self,
        items: Vec<I>,
        function: F,
    ) -> Result<Vec<O>, String> {
        if self.shared.queue.lock().unwrap().stopped {
            return Err("CPU pool is stopped".into());
        }
        let count = items.len();
        if count == 0 {
            return Ok(Vec::new());
        }
        self.grow(count.saturating_sub(1));
        let function = Arc::new(function);
        let (tx, rx) = mpsc::channel();
        for (index, item) in items.into_iter().enumerate() {
            let function = function.clone();
            let tx = tx.clone();
            let job: Job = Box::new(move || {
                let value =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| function(item)))
                        .map_err(|_| "CPU task panicked".to_string());
                let _ = tx.send((index, value));
            });
            let mut q = self.shared.queue.lock().unwrap();
            if self.helpers == 0 || q.jobs.len() >= self.helpers.max(1) * 8 {
                drop(q);
                job();
            } else {
                q.jobs.push_back(job);
                drop(q);
                self.shared.ready.notify_one();
            }
        }
        drop(tx);
        let mut values: Vec<Option<O>> = (0..count).map(|_| None).collect();
        let mut remaining = count;
        let mut failure = None;
        while remaining > 0 {
            match rx.try_recv() {
                Ok((index, result)) => {
                    match result {
                        Ok(value) => values[index] = Some(value),
                        Err(e) => failure = Some(e),
                    }
                    remaining -= 1;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if !self.help() {
                        match rx.recv_timeout(Duration::from_millis(1)) {
                            Ok((index, result)) => {
                                match result {
                                    Ok(value) => values[index] = Some(value),
                                    Err(e) => failure = Some(e),
                                }
                                remaining -= 1;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(_) => return Err("CPU job channel closed".into()),
                        }
                    }
                }
                Err(_) => return Err("CPU job channel closed".into()),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(values.into_iter().map(Option::unwrap).collect())
    }
}
impl Drop for TaskPool {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().stopped = true;
        self.shared.ready.notify_all();
        for handle in self.workers.get_mut().unwrap().drain(..) {
            if handle.thread().id() != thread::current().id() {
                let _ = handle.join();
            }
        }
    }
}
