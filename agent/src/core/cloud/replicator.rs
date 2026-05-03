//! Eager-path write queue + drainer thread.
//!
//! Tier 1 eager writes: PSRAM cache update → enqueue here → drainer
//! pops, signs, PUTs to S3. On failure: exponential backoff, retry up
//! to retry_max, then demote to dead-letter. Dead-letter entries
//! surface in /api/status and a UI banner; surface-and-stop semantics
//! (no silent forever-retry).

use crate::core::cloud::client::{ObjectStore, S3Error};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PendingWrite {
    pub key: String,
    pub bytes: Vec<u8>,
    pub queued_at: Instant,
    pub retry_count: u8,
}

#[derive(Debug, Clone)]
pub struct DeadLetterEntry {
    pub key: String,
    pub bytes: Vec<u8>,
    pub retry_count: u8,
    pub last_error_at: Instant,
    pub last_error_msg: String,
}

#[derive(Clone)]
pub struct ReplicatorConfig {
    pub queue_max: u32,
    pub retry_max: u8,
    pub backoff_cap_secs: u32,
}

impl From<&crate::config::ReplicatorConfig> for ReplicatorConfig {
    fn from(c: &crate::config::ReplicatorConfig) -> Self {
        Self { queue_max: c.queue_max, retry_max: c.retry_max, backoff_cap_secs: c.backoff_cap_secs }
    }
}

#[derive(Default)]
struct ReplicatorState {
    queue: VecDeque<PendingWrite>,
    dead_letter: Vec<DeadLetterEntry>,
    last_sync_at: Option<Instant>,
    stopping: bool,
}

#[derive(Clone)]
pub struct Replicator {
    state: Arc<Mutex<ReplicatorState>>,
    cv: Arc<Condvar>,
    cfg: ReplicatorConfig,
}

impl Replicator {
    pub fn new(cfg: ReplicatorConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(ReplicatorState::default())),
            cv: Arc::new(Condvar::new()),
            cfg,
        }
    }

    /// Enqueue a write. Coalesces by key — if the same key is already
    /// pending and not yet started, replace its bytes (last-writer-wins).
    /// Blocks if queue is at queue_max until depth drops below half-cap.
    pub fn enqueue(&self, key: String, bytes: Vec<u8>) {
        let mut g = self.state.lock().unwrap();
        // Backpressure
        while g.queue.len() as u32 >= self.cfg.queue_max && !g.stopping {
            g = self.cv.wait(g).unwrap();
        }
        if g.stopping { return; }
        // Coalesce
        if let Some(existing) = g.queue.iter_mut().find(|e| e.key == key) {
            existing.bytes = bytes;
            existing.queued_at = Instant::now();
        } else {
            g.queue.push_back(PendingWrite {
                key, bytes, queued_at: Instant::now(), retry_count: 0
            });
        }
        self.cv.notify_one();
    }

    pub fn queue_depth(&self) -> usize { self.state.lock().unwrap().queue.len() }
    pub fn dead_letter(&self) -> Vec<DeadLetterEntry> { self.state.lock().unwrap().dead_letter.clone() }
    pub fn last_sync_at(&self) -> Option<Instant> { self.state.lock().unwrap().last_sync_at }

    pub fn stop(&self) {
        let mut g = self.state.lock().unwrap();
        g.stopping = true;
        self.cv.notify_all();
    }

    /// Spawn the drainer thread. Returns a JoinHandle for shutdown.
    ///
    /// Stack budget: 8 KiB. The pthread default of 3-4 KiB overflows
    /// during mbedTLS handshake (observed as "Stack overflow in task
    /// pthread"). Going larger starves the 32 KiB agent thread of
    /// internal SRAM ("Failed to create task: OutOfMemory") — internal
    /// SRAM is the limiting resource because pthread stacks live there,
    /// not in PSRAM. 8 KiB matches the proven-safe size used by the
    /// Telegram poller threads in `main.rs`, which also do TLS through
    /// mbedTLS. The handshake's heap allocations land in PSRAM (per
    /// `project_esp32_chat_broken_postmortem`), keeping stack usage in
    /// the 4-6 KiB range during TLS.
    pub fn spawn_drainer(&self, store: Arc<dyn ObjectStore>) -> thread::JoinHandle<()> {
        let state = self.state.clone();
        let cv = self.cv.clone();
        let cfg = self.cfg.clone();
        thread::Builder::new()
            .name("cloud-drainer".into())
            .stack_size(8 * 1024)
            .spawn(move || drainer_loop(state, cv, cfg, store))
            .expect("Failed to spawn cloud-drainer thread")
    }
}

fn drainer_loop(
    state: Arc<Mutex<ReplicatorState>>,
    cv: Arc<Condvar>,
    cfg: ReplicatorConfig,
    store: Arc<dyn ObjectStore>,
) {
    loop {
        // Wait for work
        let mut item = {
            let mut g = state.lock().unwrap();
            while g.queue.is_empty() && !g.stopping {
                g = cv.wait(g).unwrap();
            }
            if g.stopping && g.queue.is_empty() { return; }
            g.queue.pop_front().unwrap()
        };
        cv.notify_all(); // wake any backpressure-blocked writers

        // Attempt PUT
        match store.put(&item.key, &item.bytes) {
            Ok(()) => {
                let mut g = state.lock().unwrap();
                g.last_sync_at = Some(Instant::now());
            }
            Err(S3Error(msg)) => {
                item.retry_count += 1;
                if item.retry_count > cfg.retry_max {
                    let mut g = state.lock().unwrap();
                    g.dead_letter.push(DeadLetterEntry {
                        key: item.key,
                        bytes: item.bytes,
                        retry_count: item.retry_count - 1,
                        last_error_at: Instant::now(),
                        last_error_msg: msg,
                    });
                } else {
                    // Backoff: 2^(retry-1) seconds, capped
                    let delay = Duration::from_secs(
                        (1u32 << (item.retry_count - 1).min(10))
                            .min(cfg.backoff_cap_secs) as u64
                    );
                    thread::sleep(delay);
                    let mut g = state.lock().unwrap();
                    g.queue.push_front(item);
                    cv.notify_one();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake store that succeeds N times then fails M times then succeeds.
    struct FakeStore {
        puts: Mutex<Vec<(String, Vec<u8>)>>,
        fail_for: AtomicUsize,
        fail_msg: String,
    }
    impl FakeStore {
        fn new() -> Self { Self { puts: Mutex::new(vec![]), fail_for: AtomicUsize::new(0), fail_msg: "fake".to_string() } }
        fn fail_next_n(&self, n: usize) { self.fail_for.store(n, Ordering::SeqCst); }
        fn put_log(&self) -> Vec<(String, Vec<u8>)> { self.puts.lock().unwrap().clone() }
    }
    impl ObjectStore for FakeStore {
        fn put(&self, key: &str, bytes: &[u8]) -> crate::core::cloud::client::Result<()> {
            if self.fail_for.load(Ordering::SeqCst) > 0 {
                self.fail_for.fetch_sub(1, Ordering::SeqCst);
                return Err(S3Error(self.fail_msg.clone()));
            }
            self.puts.lock().unwrap().push((key.to_string(), bytes.to_vec()));
            Ok(())
        }
        fn get(&self, _key: &str) -> crate::core::cloud::client::Result<Vec<u8>> { unimplemented!() }
        fn delete(&self, _key: &str) -> crate::core::cloud::client::Result<()> { Ok(()) }
        fn head(&self, _key: &str) -> crate::core::cloud::client::Result<Option<u64>> { Ok(None) }
    }

    fn cfg() -> ReplicatorConfig { ReplicatorConfig { queue_max: 32, retry_max: 3, backoff_cap_secs: 1 } }

    #[test]
    fn enqueue_and_drain_single_write() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        let h = r.spawn_drainer(store.clone());
        r.enqueue("sys/MEMORY.md".to_string(), b"hello".to_vec());
        // Wait briefly for drain
        thread::sleep(Duration::from_millis(50));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("sys/MEMORY.md".to_string(), b"hello".to_vec())]);
    }

    #[test]
    fn coalesces_pending_writes_for_same_key() {
        // Writes happen faster than the drainer can drain — only the last
        // version of each key should hit the store.
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        // Don't spawn drainer yet — fill queue first
        r.enqueue("k".to_string(), b"v1".to_vec());
        r.enqueue("k".to_string(), b"v2".to_vec());
        r.enqueue("k".to_string(), b"v3".to_vec());
        assert_eq!(r.queue_depth(), 1);

        let h = r.spawn_drainer(store.clone());
        thread::sleep(Duration::from_millis(50));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("k".to_string(), b"v3".to_vec())]);
    }

    #[test]
    fn retries_then_succeeds() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        store.fail_next_n(2);
        let h = r.spawn_drainer(store.clone());
        r.enqueue("k".to_string(), b"v".to_vec());
        // Wait for retries (1s + 2s with backoff_cap_secs=1 → ~2s total)
        thread::sleep(Duration::from_millis(2500));
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![("k".to_string(), b"v".to_vec())]);
        assert!(r.dead_letter().is_empty());
    }

    #[test]
    fn promotes_to_dead_letter_after_retry_max() {
        let r = Replicator::new(cfg());
        let store = Arc::new(FakeStore::new());
        store.fail_next_n(100); // will exhaust all retries
        let h = r.spawn_drainer(store.clone());
        r.enqueue("k".to_string(), b"v".to_vec());
        thread::sleep(Duration::from_millis(5000)); // 1+1+1 = 3s of backoffs at cap
        r.stop();
        let _ = h.join();
        assert_eq!(store.put_log(), vec![]); // never succeeded
        let dl = r.dead_letter();
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0].key, "k");
        assert_eq!(dl[0].retry_count, 3);
    }

    /// Store that blocks inside put() until released. Used to hold the
    /// drainer in-flight so the queue stays at capacity for the backpressure
    /// test (without this, the drainer pops before "c" is enqueued).
    struct BlockingStore {
        release: Arc<(Mutex<bool>, Condvar)>,
    }
    impl BlockingStore {
        fn new() -> Self { Self { release: Arc::new((Mutex::new(false), Condvar::new())) } }
        fn release(&self) {
            let (lock, cv) = &*self.release;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
    }
    impl ObjectStore for BlockingStore {
        fn put(&self, _key: &str, _bytes: &[u8]) -> crate::core::cloud::client::Result<()> {
            let (lock, cv) = &*self.release;
            let mut g = lock.lock().unwrap();
            while !*g { g = cv.wait(g).unwrap(); }
            Ok(())
        }
        fn get(&self, _key: &str) -> crate::core::cloud::client::Result<Vec<u8>> { unimplemented!() }
        fn delete(&self, _key: &str) -> crate::core::cloud::client::Result<()> { Ok(()) }
        fn head(&self, _key: &str) -> crate::core::cloud::client::Result<Option<u64>> { Ok(None) }
    }

    #[test]
    fn backpressure_blocks_when_queue_full() {
        let cfg = ReplicatorConfig { queue_max: 2, retry_max: 1, backoff_cap_secs: 1 };
        let r = Replicator::new(cfg);
        let store = Arc::new(BlockingStore::new());
        let _h = r.spawn_drainer(store.clone());
        r.enqueue("a".to_string(), b"v".to_vec());
        r.enqueue("b".to_string(), b"v".to_vec());
        // Give drainer time to pop "a" (it will block inside put())
        thread::sleep(Duration::from_millis(20));
        // Queue has 1 item ("b"). Now enqueue "c" to fill it to queue_max=2.
        r.enqueue("c".to_string(), b"v".to_vec());
        // Third enqueue would block — verify in a separate thread that it
        // doesn't return immediately.
        let r2 = r.clone();
        let blocked = thread::spawn(move || {
            r2.enqueue("d".to_string(), b"v".to_vec());
        });
        thread::sleep(Duration::from_millis(100));
        assert!(!blocked.is_finished(), "fourth enqueue should block on backpressure");
        // Release the store and stop — unblocks drainer → empties queue → unblocks "d"
        store.release();
        r.stop();
        let _ = blocked.join();
    }
}
