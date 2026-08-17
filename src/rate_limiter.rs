use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct RateLimiter {
    tokens: AtomicU64,
    capacity: u64,
    refill_rate: f64,
    last_refill: Mutex<Instant>,
    cooldown_until: AtomicU64,
    needs_wait: AtomicBool,
    notify: Notify,
}

impl RateLimiter {
    pub fn new(rpc_per_minute: Option<f64>) -> Arc<Self> {
        let budget = rpc_per_minute.unwrap_or(f64::INFINITY);
        let capacity = budget.ceil() as u64;
        Arc::new(Self {
            tokens: AtomicU64::new(capacity),
            capacity,
            refill_rate: budget / 60.0,
            last_refill: Mutex::new(Instant::now()),
            cooldown_until: AtomicU64::new(0),
            needs_wait: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    #[allow(dead_code)]
    pub fn unlimited() -> Arc<Self> {
        Self::new(None)
    }

    pub async fn acquire(&self) {
        loop {
            let current_ms = now_ms();
            let cooldown = self.cooldown_until.load(Ordering::Acquire);
            if current_ms < cooldown {
                let sleep_ms = cooldown - current_ms;
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                continue;
            }
            self.maybe_refill();
            let tokens = self.tokens.load(Ordering::Acquire);
            if tokens > 0 {
                if self
                    .tokens
                    .compare_exchange(tokens, tokens - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return;
                }
                continue;
            }
            self.needs_wait.store(true, Ordering::Release);
            self.notify.notified().await;
        }
    }

    #[allow(dead_code)]
    pub fn record_flood(&self, seconds: u32) {
        let current_ms = now_ms();
        let new_cooldown = current_ms + (seconds as u64 + 1) * 1000;
        let prev = self
            .cooldown_until
            .fetch_max(new_cooldown, Ordering::AcqRel);
        if new_cooldown > prev {
            self.notify.notify_waiters();
        }
    }

    fn maybe_refill(&self) {
        if self.refill_rate <= 0.0 || self.capacity == 0 {
            return;
        }
        let tokens = self.tokens.load(Ordering::Acquire);
        if tokens >= self.capacity {
            return;
        }
        let mut last = self.last_refill.lock().unwrap();
        let elapsed = last.elapsed().as_secs_f64();
        if elapsed < 0.01 {
            return;
        }
        let refill_tokens = (elapsed * self.refill_rate) as u64;
        if refill_tokens > 0 {
            let new_tokens = tokens.saturating_add(refill_tokens).min(self.capacity);
            let _ = self.tokens.compare_exchange(
                tokens,
                new_tokens,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        *last = Instant::now();
    }

    #[allow(dead_code)]
    pub fn available_tokens(&self) -> u64 {
        self.tokens.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub fn is_limited(&self) -> bool {
        self.needs_wait.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub fn is_in_cooldown(&self) -> bool {
        self.cooldown_until.load(Ordering::Acquire) > now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_needs_wait() {
        let rl = RateLimiter::new(None);
        assert_eq!(rl.capacity, u64::MAX);
        assert_eq!(rl.available_tokens(), u64::MAX);
        assert!(!rl.is_limited());
    }

    #[test]
    fn bounded_bucket_starts_full() {
        let rl = RateLimiter::new(Some(60.0));
        assert_eq!(rl.capacity, 60);
        assert_eq!(rl.available_tokens(), 60);
    }

    #[test]
    fn bounded_bucket_whole_number() {
        let rl = RateLimiter::new(Some(30.5));
        assert_eq!(rl.capacity, 31);
    }

    #[tokio::test]
    async fn acquire_consumes_token() {
        let rl = RateLimiter::new(Some(120.0));
        assert_eq!(rl.available_tokens(), 120);
        rl.acquire().await;
        assert_eq!(rl.available_tokens(), 119);
        rl.acquire().await;
        assert_eq!(rl.available_tokens(), 118);
    }

    #[tokio::test]
    async fn acquire_unlimited_never_blocks() {
        let rl = RateLimiter::new(None);
        let start = std::time::Instant::now();
        for _ in 0..100 {
            rl.acquire().await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn acquire_stalls_when_tokens_exhausted() {
        let rl = RateLimiter::new(Some(2.0));
        rl.acquire().await;
        rl.acquire().await;
        assert_eq!(rl.available_tokens(), 0);
        let rl2 = Arc::clone(&rl);
        let handle = tokio::spawn(async move {
            rl2.acquire().await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!handle.is_finished());
        handle.abort();
    }

    #[test]
    fn record_flood_sets_cooldown() {
        let rl = RateLimiter::new(None);
        assert!(!rl.is_in_cooldown());
        rl.record_flood(10);
        assert!(rl.is_in_cooldown());
    }

    #[test]
    fn record_flood_uses_max_cooldown() {
        let rl = RateLimiter::new(None);
        rl.record_flood(5);
        let cooldown5 = rl.cooldown_until.load(Ordering::Acquire);
        rl.record_flood(20);
        let cooldown20 = rl.cooldown_until.load(Ordering::Acquire);
        assert!(cooldown20 > cooldown5);
        rl.record_flood(1);
        let still_cooldown20 = rl.cooldown_until.load(Ordering::Acquire);
        assert_eq!(still_cooldown20, cooldown20);
    }

    #[test]
    fn per_account_isolation() {
        let rl_a = RateLimiter::new(Some(10.0));
        let rl_b = RateLimiter::new(Some(10.0));
        rl_a.record_flood(60);
        assert!(rl_a.is_in_cooldown());
        assert!(!rl_b.is_in_cooldown());
    }

    #[tokio::test]
    async fn cooldown_unblocks_after_wait() {
        let rl = RateLimiter::new(Some(100.0));
        let rl2 = Arc::clone(&rl);
        rl2.record_flood(0);
        let handle = tokio::spawn(async move {
            rl2.acquire().await;
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(handle.is_finished());
    }

    #[test]
    fn unlimited_capacity_is_max() {
        let rl = RateLimiter::new(None);
        assert_eq!(rl.capacity, u64::MAX);
    }

    #[test]
    fn bounded_capacity_is_correct() {
        let rl = RateLimiter::new(Some(5.0));
        assert_eq!(rl.capacity, 5);
    }

    #[tokio::test]
    async fn concurrent_acquire_respects_capacity() {
        let rl = RateLimiter::new(Some(3.0));
        let mut handles = Vec::new();
        for _ in 0..3 {
            let rl = Arc::clone(&rl);
            handles.push(tokio::spawn(async move {
                rl.acquire().await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(rl.available_tokens(), 0);
    }
}
