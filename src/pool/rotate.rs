//! 轮换策略与一个 std-only 的随机数发生器(不引 `rand`,沿用项目"std-only"惯例)。
//!
//! [`ProxyPool`](super::ProxyPool) / [`FingerprintPool`](super::FingerprintPool) 共用 [`Rotator`]
//! 在多任务并发下挑选下一个元素;[`BrowserPool`](super::BrowserPool) 也用 [`hash_key`] 做粘性定位。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 元素轮换策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RotateStrategy {
    /// 轮询:原子自增取模,均匀分配(默认)。
    #[default]
    RoundRobin,
    /// 随机:每次随机挑一个。
    Random,
    /// 粘性:按外部 key 哈希定位,**同 key 稳定拿到同一个**(如同账号固定出口代理)。
    /// 不带 key 时回退为轮询。
    Sticky,
}

/// 一个无锁(基于原子计数 + SplitMix64)的随机数发生器。分布良好,够"挑代理/指纹"用。
pub(crate) struct Rng {
    state: AtomicU64,
}

impl Rng {
    /// 用"当前时间 + 全局扰动计数"播种,保证多个实例种子不同。
    pub(crate) fn new() -> Self {
        static PERTURB: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let p = PERTURB.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        Self {
            state: AtomicU64::new(nanos ^ p ^ 0x1234_5678_9ABC_DEF0),
        }
    }

    /// 取下一个 64 位随机数(SplitMix64,基于原子自增的计数器,无锁)。
    pub(crate) fn next_u64(&self) -> u64 {
        let z = self
            .state
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = z;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 取 `[0, n)` 范围内的一个值;`n == 0` 返回 0。
    pub(crate) fn below(&self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// FNV-1a 64 位哈希,用于粘性定位(`Sticky` 策略 / 池的 worker 粘性)。
pub(crate) fn hash_key(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 通用轮换器:按策略在 `[0, len)` 里挑一个下标。线程安全(原子游标 + 无锁 RNG)。
pub(crate) struct Rotator {
    strategy: RotateStrategy,
    cursor: AtomicU64,
    rng: Rng,
}

impl Rotator {
    pub(crate) fn new(strategy: RotateStrategy) -> Self {
        Self {
            strategy,
            cursor: AtomicU64::new(0),
            rng: Rng::new(),
        }
    }

    /// 挑一个下标;`len == 0` 返回 `None`。`key` 仅 `Sticky` 策略使用(其余忽略)。
    pub(crate) fn pick(&self, len: usize, key: Option<&str>) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let n = len as u64;
        let idx = match self.strategy {
            RotateStrategy::RoundRobin => self.cursor.fetch_add(1, Ordering::Relaxed) % n,
            RotateStrategy::Random => self.rng.below(len) as u64,
            RotateStrategy::Sticky => match key {
                Some(k) => hash_key(k) % n,
                None => self.cursor.fetch_add(1, Ordering::Relaxed) % n,
            },
        };
        Some(idx as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_is_uniform_and_wraps() {
        let r = Rotator::new(RotateStrategy::RoundRobin);
        let seq: Vec<usize> = (0..6).map(|_| r.pick(3, None).unwrap()).collect();
        assert_eq!(seq, vec![0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn empty_returns_none() {
        let r = Rotator::new(RotateStrategy::RoundRobin);
        assert_eq!(r.pick(0, None), None);
        let r = Rotator::new(RotateStrategy::Random);
        assert_eq!(r.pick(0, None), None);
    }

    #[test]
    fn sticky_same_key_same_index() {
        let r = Rotator::new(RotateStrategy::Sticky);
        let a = r.pick(7, Some("user-42")).unwrap();
        let b = r.pick(7, Some("user-42")).unwrap();
        let c = r.pick(7, Some("user-99")).unwrap();
        assert_eq!(a, b, "同 key 必须稳定命中同一下标");
        assert!(a < 7 && c < 7);
    }

    #[test]
    fn random_stays_in_range() {
        let r = Rotator::new(RotateStrategy::Random);
        for _ in 0..1000 {
            assert!(r.pick(5, None).unwrap() < 5);
        }
    }

    #[test]
    fn rng_distributes_across_buckets() {
        // SplitMix64 应当能覆盖到所有桶(非退化常量)。
        let rng = Rng::new();
        let mut seen = [false; 8];
        for _ in 0..2000 {
            seen[(rng.next_u64() % 8) as usize] = true;
        }
        assert!(seen.iter().all(|&b| b), "RNG 应覆盖全部 8 个桶");
    }
}
