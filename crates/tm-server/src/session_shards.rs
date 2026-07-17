use std::{
    collections::HashMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

/// Owns the request channels and worker threads that keep `!Send` session state
/// pinned to a deterministic OS thread.
///
/// Sender handles are deliberately not exposed: dropping the pool must disconnect
/// every receiver before joining workers so cached sessions are destroyed on their
/// owning thread.
pub(crate) struct ThreadAffineShardPool<R> {
    senders: Vec<mpsc::Sender<R>>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl<R: Send + 'static> ThreadAffineShardPool<R> {
    pub(crate) fn spawn<F, W>(
        shard_count: usize,
        thread_name_prefix: &str,
        mut worker_factory: F,
    ) -> Self
    where
        F: FnMut(usize) -> W,
        W: FnOnce(mpsc::Receiver<R>) + Send + 'static,
    {
        assert!(shard_count > 0, "shard_count must be non-zero");
        let mut pool = Self {
            senders: Vec::with_capacity(shard_count),
            workers: Vec::with_capacity(shard_count),
        };
        for shard_id in 0..shard_count {
            pool.push_worker(
                format!("{thread_name_prefix}-{shard_id}"),
                worker_factory(shard_id),
            );
        }
        pool
    }

    pub(crate) fn spawn_one(
        thread_name: &str,
        worker: impl FnOnce(mpsc::Receiver<R>) + Send + 'static,
    ) -> Self {
        let mut pool = Self {
            senders: Vec::with_capacity(1),
            workers: Vec::with_capacity(1),
        };
        pool.push_worker(thread_name.to_string(), worker);
        pool
    }

    fn push_worker(
        &mut self,
        thread_name: String,
        worker: impl FnOnce(mpsc::Receiver<R>) + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::Builder::new()
            .name(thread_name)
            .spawn(move || worker(receiver))
            .expect("session shard thread starts");
        self.senders.push(sender);
        self.workers.push(handle);
    }

    pub(crate) fn send(
        &self,
        session_id: Uuid,
        request: R,
    ) -> std::result::Result<(), mpsc::SendError<R>> {
        self.senders[shard_index(session_id, self.senders.len())].send(request)
    }
}

impl<R> Drop for ThreadAffineShardPool<R> {
    fn drop(&mut self) {
        self.senders.clear();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

pub(crate) fn session_sweep_interval(session_ttl: Duration) -> Duration {
    session_ttl
        .max(Duration::from_millis(1))
        .min(Duration::from_secs(60))
}

pub(crate) fn evict_expired_sessions<V>(
    sessions: &mut HashMap<Uuid, V>,
    session_ttl: Duration,
    now: Instant,
    last_used: impl Fn(&V) -> Instant,
) {
    sessions.retain(|_, cached| now.saturating_duration_since(last_used(cached)) < session_ttl);
}

pub(crate) fn shard_index(session_id: Uuid, shard_count: usize) -> usize {
    (session_id.as_u128() % shard_count as u128) as usize
}

#[cfg(test)]
mod tests {
    use std::{
        rc::Rc,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    struct ThreadBoundDropProbe {
        drops: Arc<AtomicUsize>,
        drop_threads: Arc<Mutex<Vec<String>>>,
        _not_send: Rc<()>,
    }

    impl Drop for ThreadBoundDropProbe {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
            self.drop_threads
                .lock()
                .unwrap()
                .push(thread::current().name().unwrap_or("unnamed").to_string());
        }
    }

    #[test]
    fn dropping_pool_joins_workers_and_destroys_thread_bound_state() {
        let drops = Arc::new(AtomicUsize::new(0));
        let drop_threads = Arc::new(Mutex::new(Vec::new()));
        let pool = ThreadAffineShardPool::<()>::spawn(2, "test-session-shard", |_| {
            let drops = Arc::clone(&drops);
            let drop_threads = Arc::clone(&drop_threads);
            move |receiver| {
                let _probe = ThreadBoundDropProbe {
                    drops,
                    drop_threads,
                    _not_send: Rc::new(()),
                };
                for () in receiver {}
            }
        });

        drop(pool);

        assert_eq!(drops.load(Ordering::SeqCst), 2);
        let mut threads = drop_threads.lock().unwrap().clone();
        threads.sort();
        assert_eq!(
            threads,
            vec!["test-session-shard-0", "test-session-shard-1"]
        );
    }

    #[test]
    fn eviction_uses_a_strict_ttl_boundary() {
        let started = Instant::now();
        let active_id = Uuid::from_u128(1);
        let expired_id = Uuid::from_u128(2);
        let mut sessions = HashMap::from([
            (active_id, started + Duration::from_millis(1)),
            (expired_id, started),
        ]);

        evict_expired_sessions(
            &mut sessions,
            Duration::from_millis(2),
            started + Duration::from_millis(2),
            |last_used| *last_used,
        );

        assert!(sessions.contains_key(&active_id));
        assert!(!sessions.contains_key(&expired_id));
        assert_eq!(
            session_sweep_interval(Duration::ZERO),
            Duration::from_millis(1)
        );
        assert_eq!(
            session_sweep_interval(Duration::from_secs(120)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn routing_is_stable_for_a_session_id() {
        assert_eq!(shard_index(Uuid::from_u128(4), 3), 1);
        assert_eq!(shard_index(Uuid::from_u128(4), 3), 1);
    }
}
