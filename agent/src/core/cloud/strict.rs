//! Strict-path writes — block on S3 confirmation, retry inline, return
//! the error to the caller after exhausting `retry_max` attempts.
//!
//! Used by callers that cannot tolerate eventual consistency: memory
//! saves (the agent expects its own writes to be durable before
//! returning), cron updates (next-run state must round-trip), and
//! config writes (`/api/config` reboots the device, so the new config
//! must be in S3 before the reboot).
//!
//! Distinct from the eager [`super::replicator::Replicator`] path,
//! which acks immediately and PUTs in the background. Both share the
//! same [`ObjectStore`] trait so a single fake covers both in tests.

use crate::core::cloud::client::{ObjectStore, S3Error};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Synchronously PUT `bytes` to `key`, retrying on failure with
/// exponential backoff (1s, 2s, 4s, ... capped at `backoff_cap_secs`).
///
/// Returns `Ok(())` on the first successful PUT, or the *last* error
/// after `retry_max + 1` total attempts. Caller surfaces the error to
/// the agent (memory tool error) or to the HTTP client (config 503).
pub fn strict_put(
    store: &Arc<dyn ObjectStore>,
    key: &str,
    bytes: &[u8],
    retry_max: u8,
    backoff_cap_secs: u32,
) -> Result<(), S3Error> {
    let mut last_err = S3Error("not attempted".to_string());
    for attempt in 0..=retry_max {
        match store.put(key, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                if attempt < retry_max {
                    let delay_secs = (1u32 << attempt.min(10)).min(backoff_cap_secs);
                    thread::sleep(Duration::from_secs(delay_secs as u64));
                }
            }
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Same shape as `replicator::tests::FakeStore` — the two test
    /// modules can't share definitions across `mod` boundaries without
    /// an extra `pub(crate)` module, so we duplicate the small struct.
    struct FakeStore {
        puts: Mutex<Vec<(String, Vec<u8>)>>,
        fail_for: AtomicUsize,
    }
    impl FakeStore {
        fn new() -> Self {
            Self {
                puts: Mutex::new(vec![]),
                fail_for: AtomicUsize::new(0),
            }
        }
        fn fail_next_n(&self, n: usize) {
            self.fail_for.store(n, Ordering::SeqCst);
        }
        fn put_count(&self) -> usize {
            self.puts.lock().unwrap().len()
        }
    }
    impl ObjectStore for FakeStore {
        fn put(&self, key: &str, bytes: &[u8]) -> crate::core::cloud::client::Result<()> {
            if self.fail_for.load(Ordering::SeqCst) > 0 {
                self.fail_for.fetch_sub(1, Ordering::SeqCst);
                return Err(S3Error("fake".to_string()));
            }
            self.puts
                .lock()
                .unwrap()
                .push((key.to_string(), bytes.to_vec()));
            Ok(())
        }
        fn get(&self, _key: &str) -> crate::core::cloud::client::Result<Vec<u8>> {
            unimplemented!()
        }
        fn delete(&self, _key: &str) -> crate::core::cloud::client::Result<()> {
            Ok(())
        }
        fn head(&self, _key: &str) -> crate::core::cloud::client::Result<Option<u64>> {
            Ok(None)
        }
    }

    #[test]
    fn strict_put_succeeds_first_try() {
        let store: Arc<dyn ObjectStore> = Arc::new(FakeStore::new());
        let res = strict_put(&store, "sys/cron.json", b"{}", 3, 1);
        assert!(res.is_ok());
    }

    #[test]
    fn strict_put_succeeds_after_two_retries() {
        let fake = Arc::new(FakeStore::new());
        fake.fail_next_n(2);
        let store: Arc<dyn ObjectStore> = fake.clone();
        // backoff_cap=1 → sleeps 1s + 1s → ~2s total. Test waits inside.
        let res = strict_put(&store, "sys/MEMORY.md", b"hello", 3, 1);
        assert!(res.is_ok(), "expected success on third try, got {:?}", res);
        assert_eq!(fake.put_count(), 1);
    }

    #[test]
    fn strict_put_returns_err_after_retry_max() {
        let fake = Arc::new(FakeStore::new());
        fake.fail_next_n(100); // never succeeds
        let store: Arc<dyn ObjectStore> = fake.clone();
        // retry_max=2 + initial = 3 attempts. backoff_cap=1 → 1s + 1s = 2s.
        let res = strict_put(&store, "sys/config.json", b"{}", 2, 1);
        assert!(res.is_err());
        assert_eq!(fake.put_count(), 0);
        assert_eq!(res.unwrap_err().0, "fake");
    }

    #[test]
    fn strict_put_zero_retries_attempts_once() {
        let fake = Arc::new(FakeStore::new());
        fake.fail_next_n(1);
        let store: Arc<dyn ObjectStore> = fake.clone();
        let res = strict_put(&store, "k", b"v", 0, 1);
        assert!(res.is_err(), "no retries → first failure is final");
    }
}
