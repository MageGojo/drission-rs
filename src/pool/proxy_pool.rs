//! 代理池:在一组代理之间按策略轮换,供并发池为每个标签/任务分配不同出口。

use std::sync::Arc;

use crate::launcher::Proxy;

use super::rotate::{RotateStrategy, Rotator};

/// 一组可轮换的代理。克隆代价低(内部 `Arc`),可在多任务并发取用,游标共享。
///
/// ```
/// use drission::prelude::*;
/// let pool = ProxyPool::new(vec![
///     Proxy::new("socks5://127.0.0.1:1080"),
///     Proxy::new("http://127.0.0.1:8888"),
/// ]).strategy(RotateStrategy::RoundRobin);
/// let _p = pool.next();              // 轮询取下一个
/// let _q = pool.for_key("acct-1");   // 粘性:同 key 固定出口(策略须为 Sticky 才稳定)
/// ```
#[derive(Clone)]
pub struct ProxyPool {
    proxies: Arc<Vec<Proxy>>,
    rotator: Arc<Rotator>,
}

impl ProxyPool {
    /// 用一组代理新建池,默认 [`RotateStrategy::RoundRobin`]。
    pub fn new(proxies: Vec<Proxy>) -> Self {
        Self::with_strategy(proxies, RotateStrategy::RoundRobin)
    }

    /// 用一组代理 + 指定策略新建池。
    pub fn with_strategy(proxies: Vec<Proxy>, strategy: RotateStrategy) -> Self {
        Self {
            proxies: Arc::new(proxies),
            rotator: Arc::new(Rotator::new(strategy)),
        }
    }

    /// 链式设置轮换策略(返回新句柄,游标重置)。
    pub fn strategy(self, strategy: RotateStrategy) -> Self {
        Self {
            proxies: self.proxies,
            rotator: Arc::new(Rotator::new(strategy)),
        }
    }

    /// 池中代理数量。
    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    /// 是否为空池。
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }

    /// 按策略取下一个代理;空池返回 `None`。
    #[allow(clippy::should_implement_trait)] // 故意用 `next` 命名以贴近"取下一个"的直觉
    pub fn next(&self) -> Option<Proxy> {
        self.rotator
            .pick(self.proxies.len(), None)
            .map(|i| self.proxies[i].clone())
    }

    /// 按 key 粘性取代理(策略为 [`RotateStrategy::Sticky`] 时同 key 稳定命中同一个);空池返回 `None`。
    pub fn for_key(&self, key: &str) -> Option<Proxy> {
        self.rotator
            .pick(self.proxies.len(), Some(key))
            .map(|i| self.proxies[i].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> ProxyPool {
        ProxyPool::new(vec![
            Proxy::new("http://a:1"),
            Proxy::new("http://b:2"),
            Proxy::new("http://c:3"),
        ])
    }

    #[test]
    fn round_robin_cycles() {
        let p = pool();
        let got: Vec<String> = (0..4).map(|_| p.next().unwrap().server).collect();
        assert_eq!(got, vec!["http://a:1", "http://b:2", "http://c:3", "http://a:1"]);
    }

    #[test]
    fn empty_pool_returns_none() {
        let p = ProxyPool::new(vec![]);
        assert!(p.is_empty());
        assert!(p.next().is_none());
        assert!(p.for_key("x").is_none());
    }

    #[test]
    fn sticky_same_key_same_proxy() {
        let p = pool().strategy(RotateStrategy::Sticky);
        let a = p.for_key("acct-7").unwrap().server;
        let b = p.for_key("acct-7").unwrap().server;
        assert_eq!(a, b);
    }

    #[test]
    fn clones_share_cursor() {
        // 克隆共享同一游标:交替取应继续轮询序列,而非各自从头。
        let p = pool();
        let q = p.clone();
        assert_eq!(p.next().unwrap().server, "http://a:1");
        assert_eq!(q.next().unwrap().server, "http://b:2");
        assert_eq!(p.next().unwrap().server, "http://c:3");
    }
}
