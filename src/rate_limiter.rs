use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

pub const PAGE_ITEMS: usize = 100;

pub fn needs_page_token(served: usize) -> bool {
    served > 0 && served.is_multiple_of(PAGE_ITEMS)
}

pub struct RateLimiter {
    tokens: AtomicU64,
    capacity: u64,
    refill_rate: f64,
    last_refill: Mutex<Instant>,
    needs_wait: AtomicBool,
    notify: Notify,
}

impl RateLimiter {
    pub fn new(rpc_per_minute: Option<f64>) -> Arc<Self> {
        let budget = match rpc_per_minute {
            Some(rate) if rate > 0.0 => rate,
            _ => f64::INFINITY,
        };
        let capacity = budget.ceil() as u64;
        Arc::new(Self {
            tokens: AtomicU64::new(capacity),
            capacity,
            refill_rate: budget / 60.0,
            last_refill: Mutex::new(Instant::now()),
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
            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            let _ = tokio::time::timeout(Duration::from_millis(100), notified).await;
        }
    }

    pub async fn acquire_for_items(&self, served: usize) {
        if needs_page_token(served) {
            self.acquire().await;
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
        let mut last = self.last_refill.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = last.elapsed().as_secs_f64();
        if elapsed < 0.01 {
            return;
        }
        let refill_tokens = (elapsed * self.refill_rate) as u64;
        if refill_tokens > 0 {
            let new_tokens = tokens.saturating_add(refill_tokens).min(self.capacity);
            if self
                .tokens
                .compare_exchange(tokens, new_tokens, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                *last = Instant::now();
                self.notify.notify_waiters();
            }
        }
    }

    #[allow(dead_code)]
    pub fn available_tokens(&self) -> u64 {
        self.maybe_refill();
        self.tokens.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub fn is_limited(&self) -> bool {
        self.needs_wait.load(Ordering::Acquire)
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

    #[tokio::test]
    async fn per_account_isolation() {
        let rl_a = RateLimiter::new(Some(10.0));
        let rl_b = RateLimiter::new(Some(10.0));
        for _ in 0..10 {
            rl_a.acquire().await;
        }
        assert_eq!(rl_a.available_tokens(), 0);
        assert_eq!(rl_b.available_tokens(), 10);
    }

    #[test]
    fn needs_page_token_fires_on_page_boundaries_only() {
        assert!(!needs_page_token(0));
        assert!(!needs_page_token(1));
        assert!(!needs_page_token(99));
        assert!(needs_page_token(100));
        assert!(!needs_page_token(101));
        assert!(needs_page_token(200));
    }

    #[tokio::test]
    async fn acquire_for_items_never_stalls_on_unlimited() {
        let rl = RateLimiter::new(None);
        for served in 1..=250 {
            rl.acquire_for_items(served).await;
        }
    }

    #[test]
    fn unlimited_capacity_is_max() {
        let rl = RateLimiter::new(None);
        assert_eq!(rl.capacity, u64::MAX);
    }

    #[tokio::test]
    async fn zero_budget_acts_as_unlimited() {
        let rl = RateLimiter::new(Some(0.0));
        assert_eq!(rl.capacity, u64::MAX);
        let start = std::time::Instant::now();
        tokio::time::timeout(Duration::from_millis(500), rl.acquire())
            .await
            .expect("zero budget must not stall acquire");
        assert!(start.elapsed() < Duration::from_millis(400));
    }

    #[tokio::test]
    async fn negative_budget_acts_as_unlimited() {
        let rl = RateLimiter::new(Some(-5.0));
        assert_eq!(rl.capacity, u64::MAX);
        tokio::time::timeout(Duration::from_millis(500), rl.acquire())
            .await
            .expect("negative budget must not stall acquire");
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

    #[tokio::test]
    async fn refill_wakes_parked_waiter() {
        let rl = RateLimiter::new(Some(120.0));
        for _ in 0..120 {
            rl.acquire().await;
        }
        assert_eq!(rl.available_tokens(), 0);
        let rl2 = Arc::clone(&rl);
        let handle = tokio::spawn(async move {
            rl2.acquire().await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!handle.is_finished());
        let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "waiter must wake within 5s");
        assert!(result.unwrap().is_ok());
    }

    #[tokio::test]
    async fn fractional_credit_accumulates() {
        let rl = RateLimiter::new(Some(30.0));
        for _ in 0..30 {
            rl.acquire().await;
        }
        assert_eq!(rl.available_tokens(), 0);
        tokio::time::sleep(Duration::from_millis(3200)).await;
        assert!(rl.available_tokens() >= 1);
        rl.acquire().await;
    }
}
