use crate::pagination::needs_page_token;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};

pub struct RateLimiter {
    tokens: AtomicU64,
    capacity: u64,
    refill_rate: f64,
    last_refill: Mutex<Instant>,
    fractional: Mutex<f64>,
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
            fractional: Mutex::new(0.0),
            needs_wait: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    #[cfg(test)]
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
                    self.needs_wait.store(false, Ordering::Release);
                    return;
                }
                continue;
            }
            self.needs_wait.store(true, Ordering::Release);
            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            self.maybe_refill();
            let tokens = self.tokens.load(Ordering::Acquire);
            if tokens > 0 {
                if self
                    .tokens
                    .compare_exchange(tokens, tokens - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.needs_wait.store(false, Ordering::Release);
                    return;
                }
                continue;
            }
            let deadline = self.token_deadline();
            let wait = deadline.saturating_duration_since(Instant::now());
            let capped = wait.min(Duration::from_millis(500));
            let _ = tokio::time::timeout(capped.max(Duration::from_millis(1)), notified).await;
        }
    }

    fn token_deadline(&self) -> Instant {
        if self.refill_rate <= 0.0 || self.capacity == 0 {
            return Instant::now() + Duration::from_secs(3600);
        }
        let tokens = self.tokens.load(Ordering::Acquire);
        let deficit = self.capacity.saturating_sub(tokens);
        if deficit == 0 {
            return Instant::now();
        }
        let frac = self.fractional.try_lock().map(|g| *g).unwrap_or(0.0);
        let needed = deficit as f64 - frac;
        if needed <= 0.0 {
            return Instant::now();
        }
        let secs = needed / self.refill_rate;
        Instant::now() + Duration::from_secs_f64(secs)
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
        let mut last = match self.last_refill.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let elapsed = last.elapsed().as_secs_f64();
        if elapsed < 0.01 {
            return;
        }
        let mut frac = match self.fractional.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        *frac += elapsed * self.refill_rate;
        let whole = *frac as u64;
        if whole == 0 {
            return;
        }
        let mut current = self.tokens.load(Ordering::Acquire);
        loop {
            if current >= self.capacity {
                return;
            }
            let add = whole.min(self.capacity - current);
            if add == 0 {
                return;
            }
            let new = current + add;
            match self
                .tokens
                .compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    *frac -= add as f64;
                    *last += Duration::from_secs_f64(add as f64 / self.refill_rate);
                    self.needs_wait.store(false, Ordering::Release);
                    self.notify.notify_waiters();
                    break;
                }
                Err(actual) => {
                    current = actual;
                    if current >= self.capacity {
                        return;
                    }
                    continue;
                }
            }
        }
    }

    #[cfg(test)]
    pub fn available_tokens(&self) -> u64 {
        self.maybe_refill();
        self.tokens.load(Ordering::Acquire)
    }

    #[cfg(test)]
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

    #[tokio::test]
    async fn needs_wait_clears_on_refill() {
        let rl = RateLimiter::new(Some(600.0));
        for _ in 0..600 {
            rl.acquire().await;
        }
        assert_eq!(rl.available_tokens(), 0);
        rl.needs_wait.store(true, Ordering::Release);
        assert!(rl.is_limited());
        tokio::time::sleep(Duration::from_millis(200)).await;
        rl.maybe_refill();
        assert!(!rl.is_limited());
        assert!(rl.available_tokens() >= 1);
    }

    #[tokio::test]
    async fn needs_wait_clears_on_acquire() {
        let rl = RateLimiter::new(Some(120.0));
        rl.needs_wait.store(true, Ordering::Release);
        assert!(rl.is_limited());
        rl.acquire().await;
        assert!(!rl.is_limited());
    }

    #[tokio::test]
    async fn fractional_remainder_accumulates_across_refills() {
        let rl = RateLimiter::new(Some(10.0));
        for _ in 0..10 {
            rl.acquire().await;
        }
        assert_eq!(rl.available_tokens(), 0);
        *rl.fractional.try_lock().unwrap() = 0.0;
        *rl.last_refill.try_lock().unwrap() = Instant::now() - Duration::from_millis(150);
        rl.maybe_refill();
        assert_eq!(rl.available_tokens(), 0);
        let frac = *rl.fractional.try_lock().unwrap();
        assert!(frac > 0.0, "remainder should accumulate, got {frac}");
    }
}
