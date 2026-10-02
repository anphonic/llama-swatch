//! Exponential backoff: base, 2×base, 4×base, … capped.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self { base, cap, next: base }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(self.cap);
        d
    }

    pub fn reset(&mut self) {
        self.next = self.base;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_to_cap_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
        let seq: Vec<u64> = (0..6).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(seq, [2, 4, 8, 16, 30, 30]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 2);
    }
}
